// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The InterOS Agent Manager: a native portrait phone app.
//!
//! WHAT THIS IS: a Ring-3 supervisor app whose whole job is to report what
//! the device actually holds. It is not a dashboard, it is not a console, and
//! it does not simulate a platform. Every number on every screen came out of
//! a query the kernel answered, and every screen that could not get an answer
//! says which of the three things happened instead.
//!
//! THE SHAPE: one column, a status bar, a segmented control of five surfaces,
//! a scrollable body, and a bottom action bar. That is the shape the platform's
//! other apps use on a 720x1440 logical screen, and the metrics come from
//! the platform's own design system -- `interos_std::design::theme` for the
//! roles, the type scale, the spacing, the radii and the touch minimum, and
//! `interos_std::design::components` for the card and row treatments.
//!
//! THE THEME: `Theme::NEUTRAL` -- black, white and neutral grey. The chrome
//! carries no hue at all. Colour appears in exactly three places: the app's
//! own mark, the identity of the selected model, and a status the user has to
//! be able to see (allowed, denied, unavailable).
//!
//! WHAT THIS APP DOES NOT CLAIM:
//! - It does not claim a system-wide integration. The launcher-to-service
//!   path, the authorisation and the retrieval are not wired up, and the
//!   Runtime surface says so in the platform's own terms.
//! - It does not claim a model is installed because a vendor sells one. The
//!   catalogue's install state is a measurement.
//! - It does not print a passing self-test it did not run. Every row of the
//!   self-test holds the value the platform's own function returned.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use interos_std as std;
use std::design::accessibility::{DynamicType, UserTextScale};
use std::design::components;
use std::design::font;
use std::design::monochrome::{self, Icon};
use std::design::primitives::Rect;
use std::design::scroll::ScrollState;
use agent_manager::theme::THEME;

use agent_manager::{
    build_embed_request, embed_attribution, format_threshold, format_u32, granted_capability_count,
    load_saved_provider, probe_capability_grants, providers, read_embed_reply,
    resolve_inference_service, save_preferred_provider, service_name, AgentTool, AgentToolOutcome,
    CapabilityGrant, ChatRole, ChatSession, ConsentOutcome, DeviceState, EmbedProbe, InstallState,
    ModelProbeStatus, PackageInstallOutcome, PackageRemoveOutcome, ProbeState, ProbeStatus,
    Provider, RegistryPackageState, RuntimeBinding, RuntimeDiagnostics, SearchConfiguration,
    SelfTest, ServiceState, Surface, ToolAuthority, transact_package_install,
    transact_package_remove, CAPABILITY_COUNT, CHAT_PROMPT_MAX, CHAT_TEXT_MAX,
    EMBED_NOTICE_TEXT_MAX, MAILBOX_PAYLOAD_MAX, MATRYOSHKA_DEFAULT_DIM, MAX_RESULTS_DEFAULT,
    PROBE_REQUEST_TEXT_MAX, REQUEST_HEADER_BYTES, SIMILARITY_DEFAULT_Q15, TOOL_LABEL_MAX,
};

// -----------------------------------------------------------------------------
// The screen
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// The theme this app draws with.
///
/// WHY a constant and not a lookup: the theme is a property of the app's
/// identity, not of a setting the user may change underneath it. The neutral
/// variant is the one the design system offers for an interface whose colour
/// is reserved.

/// The horizontal gutter the platform's other apps use.
const GUTTER: usize = 24;
/// The width of the content column.
const CONTENT_W: usize = W - 2 * GUTTER;

/// The height of the platform's band, taken from the platform.
const STATUS_BAR_HEIGHT: usize = std::design::viewport::STATUS_BAR_HEIGHT;
/// The first row this app's own chrome may paint in.
const HEADER_TOP: usize = std::design::viewport::HEADER_TOP;
/// The app bar's height.
///
/// WHY this band is the app's bar and not the platform's status bar: the
/// platform's status bar owns the top inset and this app's bar is what lives
/// under it -- the way out, the title and the two controls the showcases put
/// there.
///
/// WHY it reaches to 112: the way out is the platform's own control, and the
/// control is fixed at the row below the band and forty-eight tall. A bar of
/// the height this file used to carry cut the control in half, because the
/// segmented control beneath it answered the pixels the bar had given up.
const APPBAR_H: usize = 112;
/// The size of an app bar icon button.
const ICON_BTN: usize = 44;
/// The gap between two app bar icon buttons.
const ICON_BTN_GAP: usize = 8;
/// The app bar's title's left edge.
const APPBAR_TITLE_X: usize = 88;
/// The app bar's title's width, in pixels.
///
/// WHY it is solved from the controls and not written down: the two buttons to
/// the title's right are themselves placed from `W`, `GUTTER`, `ICON_BTN` and
/// `ICON_BTN_GAP`, and a title with no bound put its last letters on the search
/// button's circle. Deriving the span from the same numbers the rects use is
/// what keeps the word and the chrome it stops before in agreement.
const APPBAR_TITLE_MAX_W: usize =
    W - GUTTER - ICON_BTN * 2 - ICON_BTN_GAP - APPBAR_TITLE_X - ICON_BTN_GAP;
/// The scratch a surface name is trimmed into before the bar paints it.
///
/// WHY a named size and not a page of zeros: this runs for every pixel of the
/// bar, and a buffer wider than the longest name would be paid for once per
/// pixel for letters that never occur. The names are one word each.
const APPBAR_TITLE_BUF: usize = 32;
/// The row the app bar's controls are centred on.
const APPBAR_ROW: usize = HEADER_TOP + std::design::back_button::SIZE / 2;
/// The top of an app bar icon button.
///
/// WHY it is not the bar's own middle: the bar now carries the band's rows as
/// well, so a control centred on the whole bar sits above the platform's back
/// button instead of beside it.
const ICON_BTN_TOP: usize = APPBAR_ROW - ICON_BTN / 2;
/// The app bar's title's line box top.
///
/// WHY it is solved from the baseline rather than from the box: the box is
/// taller than the forty-eight the control stands in, so centring the box
/// would leave the bar's row and pushing it down to clear the band drops the
/// letters below the control. Centring the caps -- half a cap height above the
/// row and half below -- is what lines the word up with the button next to it.
///
/// WHY it is a `const fn` of the level and then a run-time face: the cap height
/// and the ascent it is solved from both move with the person's text size, so a
/// `const` here would be the platform's row for every level -- and the tab band
/// beneath it, and the body beneath that, are all derived from where this
/// line's ink ends.
const fn appbar_title_y_at(level: u8) -> usize {
    APPBAR_ROW - ascent_at(SCALE_TITLE, level) + cap_at(SCALE_TITLE, level) as usize / 2
}
#[inline]
fn appbar_title_y() -> usize {
    appbar_title_y_at(level())
}
/// The last row the app bar's title ink takes: its box's top plus the ascent the
/// caps sit on, since a surface's name is written with no descender.
const fn appbar_title_ink_bottom_at(level: u8) -> usize {
    appbar_title_y_at(level) + ascent_at(SCALE_TITLE, level)
}
/// The first row the title's caps take: the baseline less the cap height the
/// rung draws them at.
const fn appbar_title_ink_top_at(level: u8) -> usize {
    appbar_title_y_at(level) + ascent_at(SCALE_TITLE, level) - cap_at(SCALE_TITLE, level) as usize
}
/// The air the authored bar leaves under the title's line box, solved from the
/// authored pair rather than written down: the bar was tall enough to hold the
/// rung's whole box, and this is what was left over.
const APPBAR_TITLE_AIR: usize =
    APPBAR_H - (appbar_title_y_at(PLATFORM_LEVEL) + line_at(SCALE_TITLE, PLATFORM_LEVEL));
/// The bar's own foot at a level: the authored band, or the title's line box
/// plus its air when the rung has grown the box past the band.
///
/// WHY a run-time face and not a wider `const`: the band's number is the layout
/// the platform's contract reads, and the only way to keep both that number and
/// a name that fills it is to take the larger of the two at draw time -- which
/// leaves the authored pixels exactly as they were and gives the raised ink the
/// rows it needs.
const fn appbar_bottom_at(level: u8) -> usize {
    let grown = appbar_title_y_at(level) + line_at(SCALE_TITLE, level) + APPBAR_TITLE_AIR;
    if grown > APPBAR_H {
        grown
    } else {
        APPBAR_H
    }
}
#[inline]
fn appbar_bottom() -> usize {
    appbar_bottom_at(level())
}
// WHY checked here: the bar's row was solved from the SDK's ladder before this
// file read the setting at all, and the derivation is only honest while the
// authored level answers that same authored formula.
const _: () = assert!(appbar_title_y_at(PLATFORM_LEVEL)
    == APPBAR_ROW
        - font::ascent_for_scale(SCALE_TITLE)
        + font::cap_for_scale(SCALE_TITLE) as usize / 2);
const _: () = assert!(appbar_bottom_at(PLATFORM_LEVEL) == APPBAR_H);
/// WHY checked here: the name is the largest thing on the bar and the segmented
/// control is the first thing under it, so a raised rung that pushed the name's
/// last inked row into the segments' first row would paint a title through the
/// control a person taps -- and this platform paints no clip. The name's caps
/// also may not climb into the rows the platform's own band owns.
const fn appbar_title_clears_the_tabs_at_every_level() -> bool {
    let mut level = 0;
    while level <= 3 {
        if appbar_title_ink_bottom_at(level) >= tab_y_at(level) {
            return false;
        }
        if appbar_title_ink_top_at(level) < STATUS_BAR_HEIGHT {
            return false;
        }
        if appbar_bottom_at(level) > tab_y_at(level) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(appbar_title_clears_the_tabs_at_every_level());

/// The floating action button's radius.
const FAB_R: usize = 28;
/// The floating action button's margin from the body's edge.
const FAB_MARGIN: usize = 16;
/// The padding inside a chat bubble.
const BUBBLE_PAD: usize = 12;
/// The chat surface's composer band.
///
/// The authored band, and its floor: the field it holds follows the type, so at
/// the larger levels the band is grown to the field plus the same air
/// (`composer_h`), never shrunk below the number the layout was drawn with.
const COMPOSER_H: usize = 76;
/// The composer's field height.
///
/// WHY it follows the type rather than being a number: the platform's
/// type scale names a line box, and a field that is shorter than the line
/// it holds cannot hold it. A fixed height here broke the moment the
/// ladder's rungs moved, because the arithmetic underflowed.
///
/// WHY it moves with the published rung for the same reason: the field holds a
/// body-rung line, and a field that keeps the Normal box while the ink is
/// raised to Large is the same arithmetic mistake one rung further down.
const fn composer_field_h_at(level: u8) -> usize {
    line_at(SCALE_BODY, level) + 28
}
#[inline]
fn composer_field_h() -> usize {
    composer_field_h_at(level())
}
/// The air the authored band leaves around the field it centres, solved from
/// the authored pair rather than written down, so the band can grow with the
/// field without the field ever sitting in a band of a different proportions.
const COMPOSER_AIR: usize = COMPOSER_H - composer_field_h_at(PLATFORM_LEVEL);
/// The composer band at the published level: the authored band, or the field
/// plus its air when the rung has grown the field past the band.
///
/// WHY a run-time face rather than a compile-time bound: the band's floor is a
/// layout number the platform's contract reads, and the only honest way to keep
/// both it and the field is to take the larger of the two at draw time.
const fn composer_h_at(level: u8) -> usize {
    let grown = composer_field_h_at(level) + COMPOSER_AIR;
    if grown > COMPOSER_H {
        grown
    } else {
        COMPOSER_H
    }
}
#[inline]
fn composer_h() -> usize {
    composer_h_at(level())
}
/// The composer's send button radius.
const COMPOSER_SEND_R: usize = 24;

/// The segmented control's band.
///
/// WHY it follows the bar rather than sitting at a number of its own: the bar
/// grew when it took the platform's back control, and a band pinned at the old
/// bottom was drawn over the control it no longer clears.
///
/// WHY the same rule is now a function of the rung: the bar's title is centred
/// caps, so a raised rung pushes the name's last inked row down, and a band
/// that stayed at 120 while the caps ran past it would be drawn under the name
/// it is supposed to sit clear of. The eight rows of air the authored layout
/// gives the control are re-applied to the bar's own foot at every level, so the
/// band moves only when the ink needs it to and the platform's pixels do not.
const fn tab_y_at(level: u8) -> usize {
    appbar_bottom_at(level) + 8
}
#[inline]
fn tab_y() -> usize {
    tab_y_at(level())
}
const TAB_Y: usize = 120;
const TAB_H: usize = 44;
const TAB_GAP: usize = 5;

/// The scrollable body's band.
///
/// The authored top; `body_top()` is the same band after the tab row has moved
/// for the rung, with the same sixteen rows of air the authored layout gives it.
const BODY_TOP: usize = 180;
const fn body_top_at(level: u8) -> usize {
    tab_y_at(level) + TAB_H + 16
}
#[inline]
fn body_top() -> usize {
    body_top_at(level())
}
/// WHY at compile time: the three bands above are written as numbers so the
/// platform's layout contract can read them, and a number is only honest while
/// it still matches the geometry the controls are actually placed from. The two
/// checks beside it say the derivation answers those same numbers at the level
/// the numbers were authored for, and that raising a rung never pulls the body
/// down past the action bar it scrolls inside.
const _: () = assert!(
    APPBAR_H == HEADER_TOP + std::design::back_button::SIZE + 8
        && TAB_Y == APPBAR_H + 8
        && BODY_TOP == TAB_Y + TAB_H + 16
        && tab_y_at(PLATFORM_LEVEL) == TAB_Y
        && body_top_at(PLATFORM_LEVEL) == BODY_TOP
);
const fn bands_clear_the_action_bar_at_every_level() -> bool {
    let mut level = 0;
    while level <= 3 {
        if body_top_at(level) >= BODY_BOTTOM {
            return false;
        }
        if tab_y_at(level) + TAB_H >= body_top_at(level) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(bands_clear_the_action_bar_at_every_level());
const BODY_BOTTOM: usize = 1284;
const BODY_H: usize = BODY_BOTTOM - BODY_TOP;
/// The body's viewport at the published level: the authored band's floor, less
/// the rows the raised type has given to the bar and the segmented control above
/// it.
///
/// WHY the scroll state is built from this and not from `BODY_H`: the band's
/// height is how much of the card stack a person can reach without scrolling, and
/// a viewport that keeps the authored number after the band has moved reports a
/// body that ends below the action bar -- a scroll range with rows in it that no
/// pixel of the screen shows.
#[inline]
fn band_height() -> usize {
    BODY_BOTTOM.saturating_sub(body_top())
}
// WHY checked here: the authored body's height is the platform's case, and the
// band only narrows -- it never reaches past the action bar it scrolls inside.
const _: () = assert!(band_height_at(PLATFORM_LEVEL) == BODY_H);
const fn band_height_at(level: u8) -> usize {
    BODY_BOTTOM.saturating_sub(body_top_at(level))
}
/// The bottom action bar's band.
const ACTION_TOP: usize = 1284;
const BUTTON_Y: usize = 1300;
const BUTTON_H: usize = 52;
const BUTTON_GAP: usize = 8;
const RESULT_Y: usize = 1364;

/// The action bar's row, described once from the numbers above.
///
/// WHY a description and not two copies of the arithmetic: the bar painted a
/// button at `GUTTER + index * (width + BUTTON_GAP)` with
/// `width = (CONTENT_W - (live - 1) * BUTTON_GAP) / live`, and its hit test
/// recomputed both. That is the same shape of mistake the tab row made before
/// `test_the_tab_row_derives_its_geometry_from_the_surface_count` closed it -- two
/// places holding one geometry, and the plate the reader saw and the strip that
/// answered a tap drifted apart. The row is now written once and read by the
/// painter and by the tap, so the embedding probe's control cannot be drawn
/// somewhere its press does not answer.
const fn action_row(live: usize) -> agent_manager::ControlRow {
    agent_manager::ControlRow {
        x: GUTTER,
        w: CONTENT_W,
        y: BUTTON_Y,
        h: BUTTON_H,
        gap: BUTTON_GAP,
        live,
    }
}

/// Which live plate in the bar the embedding probe takes.
///
/// WHY an index and not a label match: the bar's plates are laid out in the order
/// the surface lists them, and the tap returns that same order, so the probe's
/// plate is the one the surface put at this position. The gate below checks the
/// two derivations land on it.
const PROBE_BUTTON: usize = 1;

/// WHY checked here: the probe's control is only a control if the plate the
/// painter drew is the plate the tap answers, and a phone control a finger cannot
/// hit is a control that does not exist. The Search surface holds two live plates
/// -- re-read the configuration, and the probe -- so this is the row it is drawn
/// in, at the height the bar actually uses.
const fn probe_band_agrees() -> bool {
    let row = action_row(2);
    let plate = row.slot(PROBE_BUTTON);
    match row.tapped(plate.x + plate.w / 2, plate.y + plate.h / 2) {
        Some(PROBE_BUTTON) => true,
        _ => false,
    }
}
const _: () = assert!(probe_band_agrees());
const _: () = assert!(action_row(2).slot(PROBE_BUTTON).h >= std::design::touch::MIN_TARGET);
const _: () = assert!(action_row(2).slot(PROBE_BUTTON).w >= std::design::touch::MIN_TARGET);
const _: () = assert!(action_row(2).slot(PROBE_BUTTON).y == BUTTON_Y);

/// The home indicator: the mark the shell paints, and the box that answers a
/// press on it.
///
/// Both used to be authored here: a 160x5 pill at row 1420, answered by every
/// column from 200 to 520 and by a row condition -- `y >= GESTURE_Y - 20 ||
/// y >= 1395` -- whose first term could never be the binding one, so the press
/// claimed 321 columns and stopped 25 rows above the ink it stood for.
const HOME_PILL: std::design::primitives::Rect = std::design::home_band::PILL;
/// The first row of the answering box: the last row the action band's own ink
/// takes, plus its air.
///
/// WHY it is authored rather than measured at draw time: it is the one number
/// both halves of the action band agree on -- the result line is stamped so its
/// ink ends here (see `result_y_at`), and the box begins here, so a content row
/// of this app can never answer as the exit gesture.
const HOME_HIT_TOP: usize = RESULT_Y + line_at(SCALE_CAPTION, PLATFORM_LEVEL) + 4;
/// WHY `hit_from` and not the SDK's default box: the last action's result line
/// paints down to the caption rung's whole line box plus its air, two rows
/// past the default box's first, so the box begins where this app's own ink stops.
const HOME_HIT: std::design::primitives::Rect = std::design::home_band::hit_from(HOME_HIT_TOP);

/// The last row the buttons take: the result line is stamped below it.
const fn buttons_bottom() -> usize {
    BUTTON_Y + BUTTON_H
}
/// The row the last action's result line is stamped on.
///
/// WHY it moves and the answering box does not: the caption's line box grows
/// with the published rung, and the box the ink stands in cannot be allowed to
/// run under the home gesture's first row. So the ink is lifted inside the
/// action band, where there is room, rather than pushing the answering box down
/// past the touch floor its own gate names -- and where lifting it would put it
/// on the buttons, the band's own floor holds it there instead.
const fn result_y_at(level: u8) -> usize {
    let wanted = HOME_HIT_TOP - line_at(SCALE_CAPTION, level) - 4;
    if wanted < buttons_bottom() {
        buttons_bottom()
    } else {
        wanted
    }
}
#[inline]
fn result_y() -> usize {
    result_y_at(level())
}

/// WHY checked here: the answering box still clears the platform's touch floor,
/// the mark stands inside the box it is drawn for, and nothing above the box is
/// both this app's ink and a way out of the app.
const _: () = assert!(HOME_HIT.y >= RESULT_Y + line_at(SCALE_CAPTION, PLATFORM_LEVEL));
const _: () = assert!(HOME_HIT.y + HOME_HIT.h == H);
const _: () = assert!(HOME_HIT.h >= 40);
const _: () = assert!(HOME_PILL.y >= HOME_HIT.y && HOME_PILL.y + HOME_PILL.h <= H);
const _: () = assert!(HOME_HIT.x <= HOME_PILL.x && HOME_HIT.x + HOME_HIT.w >= HOME_PILL.x + HOME_PILL.w);
/// WHY checked here: the gates above are the box's own geometry, and the box only
/// means anything if the row this app stamps its result on -- at the rung the
/// person chose, not at the authored one -- ends above the box's first row and
/// below the buttons it shares the band with. The four rows of air the authored
/// layout left above the ink are what shrinks first, because the box's floor and
/// the buttons' floor are the two bounds that cannot move: at the largest rung
/// the line sits on the buttons' last row and its box ends clear of the gesture's
/// first, so no row of this app's own text is answered as a way out.
const fn result_line_holds_at_every_level() -> bool {
    let mut level = 0;
    while level <= 3 {
        if result_y_at(level) < buttons_bottom() {
            return false;
        }
        if result_y_at(level) + line_at(SCALE_CAPTION, level) > HOME_HIT.y {
            return false;
        }
        level += 1;
    }
    true
}
// WHY apart: the clamp exists for the larger rungs, and the authored row and the
/// authored air have to survive it -- the platform's pixels are the case both of
/// these gates are written against.
const _: () = assert!(result_y_at(PLATFORM_LEVEL) == RESULT_Y);
const _: () = assert!(
    result_y_at(PLATFORM_LEVEL) + line_at(SCALE_CAPTION, PLATFORM_LEVEL) + 4 == HOME_HIT.y
);
const _: () = assert!(result_line_holds_at_every_level());

// WHY the action bar starts where the body ends: the two bands share an
// edge, and a gap between them would be a strip of page that belongs to
// neither. The compiler checks the two numbers agree.
const _: () = assert!(ACTION_TOP == BODY_BOTTOM);

// The body model.
const CARD_PAD_X: usize = 20;
const CARD_PAD_TOP: usize = 22;
const CARD_PAD_BOTTOM: usize = 18;
const CARD_GAP: usize = 14;
/// The height of a card's heading row.
///
/// WHY the heading is given a row of its own: the heading is the name of
/// the thing the rows under it belong to, and a name that shares a line
/// with a measurement is a measurement.
///
/// WHY it is the section rung's own line box and not a number written down: a
/// heading painted at the section cap reserves the section rung's line box from
/// the row it is stamped on, and forty was the cell's, not the rung's. The
/// gap under the heading happened to be wide enough to swallow the difference,
/// so nothing on screen disagreed with the number -- and a row that is shorter
/// than the ink it owns is only ever saved by luck, because this platform
/// paints no clip and a rung that moves lands the surplus on the first line.
///
/// WHY it is a function of the rung as well: the heading's ink is now drawn at
/// the published level, so a heading row that kept the authored box would be
/// the same luck one rung further -- the surplus lands on the first measurement
/// under it. The cards' heights are placed at build time from this number, and
/// the body scrolls, so a taller stack reaches the person instead of being cut.
const fn title_row_h_at(level: u8) -> usize {
    line_at(SCALE_SECTION, level)
}
const TITLE_GAP: usize = 10;
/// The height of a row.
///
/// WHY a row is a touch target's height and not a line's height: a row a
/// person can read but cannot press is a label, and the platform's
/// minimum touch target is forty-four pixels.
/// A row's height. It follows the type for the same reason the composer's
/// field does.
const fn row_h_at(level: u8) -> usize {
    line_at(SCALE_BODY, level) + 20
}
#[inline]
fn row_h() -> usize {
    row_h_at(level())
}
/// The row a card's first line sits on: its heading's whole plate plus the gap.
const fn lines_top_at(level: u8) -> usize {
    CARD_PAD_TOP + title_row_h_at(level) + TITLE_GAP
}
#[inline]
fn lines_top() -> usize {
    lines_top_at(level())
}
// WHY checked here: a heading plate and the first row under it are the two
// numbers that used to be `const`s in agreement; at every selectable level the
// plate still holds its own ink and the card's pad still clears it.
const _: () = assert!(title_row_h_at(PLATFORM_LEVEL) == std::design::typography::SECTION_LINE_PX);
const _: () = assert!(row_h_at(PLATFORM_LEVEL) == std::design::typography::BODY_LINE_PX + 20);
const _: () = assert!(lines_top_at(PLATFORM_LEVEL) == CARD_PAD_TOP + std::design::typography::SECTION_LINE_PX + TITLE_GAP);
const fn heading_plates_hold_at_every_level() -> bool {
    let mut level = 0;
    while level <= 3 {
        // The heading's ink is its cap on the section rung's baseline, and the
        // plate reserves the rung's whole line box, so the plate has to be at
        // least as tall as the ascent the caps sit on.
        if title_row_h_at(level) < ascent_at(SCALE_SECTION, level) {
            return false;
        }
        // A row carries the body rung's ink and the caption's chip, and it is
        // the row a tap is answered on: never below the platform's floor.
        if row_h_at(level) < std::design::touch::MIN_TARGET {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(heading_plates_hold_at_every_level());

/// The label column's width in pixels.
/// The label column's width.
///
/// WHY 216 and not 300: the longest label the app uses is eighteen
/// characters, and a column wide enough for the longest label is a
/// column with room to spare. The old width spent a quarter of the
/// content column on white space, which is what made the rows read
/// as a table rather than as a phone's list.
const LABEL_W: usize = 216;
/// The gap between the label column and the value column.
///
/// WHY the gap is not zero: the face is proportional, and a label that
/// exactly fills its column touches the value beside it. A phone list
/// needs a gutter.
const LABEL_GAP: usize = 8;
/// The value column's left edge, in absolute x.
const VALUE_X: usize = GUTTER + CARD_PAD_X + LABEL_W + LABEL_GAP;
/// The value column's width in pixels.
const VALUE_W: usize = GUTTER + CONTENT_W - CARD_PAD_X - VALUE_X;
/// The width of a wide line's text.
const WIDE_W: usize = GUTTER + CONTENT_W - CARD_PAD_X - (GUTTER + CARD_PAD_X);

/// The longest run the outline face will draw, in bytes.
///
/// WHY the app knows the face's limit: a run the face refuses is drawn by
/// the bitmap face at the bitmap's whole-cell size, and a paragraph drawn
/// in a different face from the label above it is a paragraph that looks
/// like it belongs to another screen.
const MAX_RUN_BYTES: usize = std::design::outline_text::KEY_MAX;
/// How many physical rows a wide line can have.
///
/// WHY twenty and not eight: a wide line's rows are found by measuring, and
/// the widest glyph the face has advances thirteen pixels at the caption
/// rung, so the column carries no fewer than the contract's measured floor of
/// forty bytes a row however the letters fall. The transcript holds the
/// service's whole seven-hundred-and-sixty-eight-byte notice window, and
/// twenty rows of the floor paint it whole: 20 * 40 = 800 >= 768. Eight rows
/// painted only 320 of the bytes the transcript kept, and the screen cut the
/// platform's own explanation of the model file mid-word.
const WIDE_ROWS: usize = 20;

// The type scale rungs the app draws with.
//
// WHY two rungs and not seven: the guest's bitmap face rasterises a 16x24
// cell at integer scales only, so the sizes the face can actually draw are
// 24px and 48px. The platform's own apps build the hierarchy between the
// rungs that are not expressible out of spacing, weight and ink, and so
// does this one: the rows are the 24px rung, the headings are the 48px
// rung, and the difference between a heading and a row is the weight and
// the ink as much as it is the size.
/// The four rungs this app draws on, named so a plate and the ink inside it can
/// be asked for the same rung at the same text-size level.
const SCALE_CAPTION: usize = std::design::typography::CAPTION_SCALE;
const SCALE_BODY: usize = std::design::typography::BODY_SCALE;
const SCALE_SECTION: usize = std::design::typography::SECTION_SCALE;
const SCALE_TITLE: usize = std::design::typography::TITLE_SCALE;

/// The text-size level this layout was authored against.
///
/// WHY it is named: every box below is derived twice -- once as a `const fn` of
/// a level, so the compile-time gates can still prove a plate holds its ink at
/// every selectable level, and once at run time through the published rung, so
/// the pixels a person actually gets follow the setting they chose. The gates
/// and the identity checks are written against this level, which is what keeps
/// the app's pixels at Normal exactly the pixels it drew before it read the
/// setting at all.
const PLATFORM_LEVEL: u8 = font::TEXT_SCALE_NORMAL;

/// The rung's cap height at a level, and the two boxes that go with it.
///
/// WHY these are `const fn`s of the level and not [`font::cap_for_scale`]: the
/// ladder `cap_for_scale` is a compile-time ladder, called from const contexts
/// across the tree, and it can never see a setting a person changes. The
/// runtime faces (`font::cap_for_scale_user` and family) read the published
/// rung instead, and a layout that moves its ink has to move its plate by the
/// same rung -- which is the half of this the app used to leave out.
const fn cap_at(scale: usize, level: u8) -> u32 {
    font::cap_for_user_scale(scale, level)
}
const fn line_at(scale: usize, level: u8) -> usize {
    font::line_height_for_user_scale(scale, level)
}
const fn ascent_at(scale: usize, level: u8) -> usize {
    font::ascent_for_user_scale(scale, level)
}

/// The rung this process draws at, as published at startup.
#[inline]
fn level() -> u8 {
    font::user_text_scale()
}

/// Reads the person's text size from the settings store and hands it to the
/// renderer, answering whether the rung actually moved.
///
/// WHY here and why once before the first frame and again on every wake: every
/// box in this file -- a row, a chip, a sheet row, the composer's field, the
/// bar's title and the home gesture's answering box -- is derived from the rung
/// at draw time, so the rung has to be published before the first of them is
/// placed and it must not change underneath a body that was already wrapped. A
/// body wrapped for the rung the person left is the clipped-text defect, and a
/// rung read only at startup is that defect arriving from the other side: the
/// size is changed in Settings while this screen is alive, and a rung nobody
/// re-reads shows up -- if at all -- on the next cold open. Once per wake is one
/// settings query per frame of events, never one per glyph.
///
/// WHY a read of the store rather than a hardcoded rung: the size belongs to the
/// person, and it is the same value Settings writes when they move the slider.
///
/// WHY nothing is allocated: the read is into a fixed buffer on this frame, the
/// way the platform's own Settings surface reads it.
fn publish_text_scale() -> bool {
    let before = level();
    let mut buf = [0u8; 32];
    if let Ok(n) = std::settings_get("display.text_scale", &mut buf) {
        if let Some(scale) = UserTextScale::from_settings_value(&buf[..n]) {
            DynamicType::new(scale).publish();
        }
    }
    level() != before
}

// WHY checked here: the app's boxes are authored against the ladder, and the
// derivation above is only a faithful restatement of it if every rung answers
// the platform's own number at the platform's level. Each of these is the value
// the file used to read straight out of `typography`.
const _: () = assert!(cap_at(SCALE_CAPTION, PLATFORM_LEVEL) == std::design::typography::CAPTION_CAP_PX);
const _: () = assert!(cap_at(SCALE_BODY, PLATFORM_LEVEL) == std::design::typography::BODY_CAP_PX);
const _: () = assert!(cap_at(SCALE_SECTION, PLATFORM_LEVEL) == std::design::typography::SECTION_CAP_PX);
const _: () = assert!(cap_at(SCALE_TITLE, PLATFORM_LEVEL) == std::design::typography::TITLE_CAP_PX);
const _: () = assert!(line_at(SCALE_CAPTION, PLATFORM_LEVEL) == std::design::typography::CAPTION_LINE_PX);
const _: () = assert!(line_at(SCALE_BODY, PLATFORM_LEVEL) == std::design::typography::BODY_LINE_PX);
const _: () = assert!(line_at(SCALE_SECTION, PLATFORM_LEVEL) == std::design::typography::SECTION_LINE_PX);
const _: () = assert!(ascent_at(SCALE_TITLE, PLATFORM_LEVEL) == font::ascent_for_scale(SCALE_TITLE));

/// The cap heights the design document names, drawn at the published level.
///
/// WHY the app names them rather than a scale multiple: the platform's
/// type scale is a ladder of cap heights, and a scale multiple is a
/// ladder of multiples. One of them can be drawn exactly and the other
/// has to be guessed at.
#[inline]
fn cap_caption() -> u32 {
    cap_at(SCALE_CAPTION, level())
}
#[inline]
fn cap_body() -> u32 {
    cap_at(SCALE_BODY, level())
}
#[inline]
fn cap_section() -> u32 {
    cap_at(SCALE_SECTION, level())
}
#[inline]
fn cap_title() -> u32 {
    cap_at(SCALE_TITLE, level())
}

/// The line heights that go with the cap heights, at the published level.
///
/// WHY they travel with the caps: a line box is the space a run reserves, and
/// ink raised to Large inside a box written for Normal is ink the plate clips
/// -- the defect class this file is being asked to close.
#[inline]
fn line_caption() -> usize {
    line_at(SCALE_CAPTION, level())
}
#[inline]
fn line_body() -> usize {
    line_at(SCALE_BODY, level())
}

/// The length of the band's clock lane, as the RTC formats it.
const CLOCK_LEN: usize = 5;

/// How many cards the body can hold.
///
/// WHY the budget is generous: a body that runs out of room drops the tail
/// of its content without saying so, and a screen that quietly hides half
/// of a provider list is a screen that lies about the device.
const MAX_CARDS: usize = 16;
/// How many lines the body can hold.
const MAX_LINES: usize = 64;
/// The longest label a line carries.
const LABEL_MAX: usize = TOOL_LABEL_MAX;
/// The longest value a line carries.
const VALUE_MAX: usize = 44;
/// The longest value a wide line carries.
///
/// WHY it is the transcript's own length: a chat message is placed on the
/// body as one wide line, and a line whose buffer is shorter than the
/// message it was handed paints an ellipsis over the end of a sentence the
/// transcript says it kept whole. The two limits are the same limit, so one
/// is written from the other and they cannot drift apart again.
const WIDE_MAX: usize = CHAT_TEXT_MAX;
/// The longest action result the app keeps.
const ACTION_LOG_MAX: usize = 120;
/// The longest card title.
const TITLE_MAX: usize = 30;
/// How long the app waits for the platform's answer before it says none came.
///
/// WHY a deadline: the person pressed send, and a transcript that stays quiet
/// while the app waits forever reports nothing about what it is waiting for.
const ANSWER_TIMEOUT_TICKS: u64 = 30_000;
/// How long one wait on the event reactor lasts before the app looks again.
const ANSWER_POLL_TICKS: u32 = 50;
/// How far a drain of the mailbox is allowed to walk.
///
/// WHY a bound: the platform keeps putting its own copies of dispatched input
/// into this app's queue, so a drain with no limit is a hang waiting on a queue
/// that never empties.
const MAILBOX_DRAIN: usize = 16;

// -----------------------------------------------------------------------------
// The body model
// -----------------------------------------------------------------------------

/// How a piece of text is coloured.
///
/// WHY an enum and not a colour: the colour comes from the theme, and a
/// screen that carried its own colours would be a second design system. The
/// tone says what the text *means*; the theme says what that means looks
/// like.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Ordinary ink.
    Plain,
    /// Secondary ink.
    Muted,
    /// Something the platform allowed.
    Good,
    /// Something the platform refused.
    Bad,
    /// Something the platform could not answer.
    Unavailable,
    /// A brand or identity mark.
    Accent,
}

impl Tone {
    /// The colour the theme gives this tone.
    fn colour(self) -> u32 {
        match self {
            Self::Plain => THEME.on_surface,
            Self::Muted => THEME.on_surface_variant,
            Self::Good => THEME.success,
            Self::Bad => THEME.error,
            Self::Unavailable => THEME.warning,
            Self::Accent => THEME.accent,
        }
    }
}

/// The tone a transcript message's text carries.
///
/// WHY the platform's own notices are muted rather than coloured as
/// warnings: a notice that reports an absence is not a failure, and
/// painting it as one would make the screen cry wolf.
fn chat_role_ink(role: ChatRole) -> Tone {
    match role {
        ChatRole::User => Tone::Plain,
        ChatRole::Assistant => Tone::Plain,
        ChatRole::Notice => Tone::Muted,
    }
}

/// The voice an answer arrives in, given what the service said about it.
///
/// WHY a refusal is a notice and not the model's words: an answer that says the
/// platform could not answer is the platform speaking, and putting it in the
/// assistant's bubble would let the transcript claim a model said it.
fn answer_role(status: u32) -> ChatRole {
    if status == std::ai::STATUS_SUCCESS {
        ChatRole::Assistant
    } else {
        ChatRole::Notice
    }
}

/// Takes the retrieval frame off the channel a search notice named.
///
/// WHY the frame is read whole: the mailbox cuts a message at sixty-four bytes
/// and a ranked result is 24 bytes of record followed by a path and a snippet,
/// so the reply that used to arrive by mailbox was always the cut one -- the
/// card then walked as many hits as the header counted and painted whatever the
/// buffer held past the bytes that had arrived. One read takes the frame, and a
/// frame shorter than the notice promised is parsed only as far as it reached.
fn streamed_search_frame(notice_bytes: &[u8], out: &mut [u8]) -> usize {
    let Some(notice) = std::ai::AiStreamNotice::from_bytes(notice_bytes) else {
        return 0;
    };
    let want = (notice.streamed_len as usize)
        .min(notice.total_len as usize)
        .min(out.len());
    match std::shm_pipe_receive(notice.pipe_id, &mut out[..want]) {
        Ok(read) => read.min(want),
        Err(_) => 0,
    }
}

/// Takes the install sentence off the channel its notice named, with what the
/// service said about it.
///
/// WHY the status travels beside the text: the service answers an install with a
/// measurement of the model namespace, and whether that measurement says the file
/// is whole is the platform's statement, not this app's. The app only has to keep
/// the two together rather than decide on its own what the bytes mean.
fn streamed_install_notice(
    notice_bytes: &[u8],
    out: &mut [u8],
) -> Option<(u32, usize, u32)> {
    let notice = std::ai::AiStreamNotice::from_bytes(notice_bytes)?;
    let want = (notice.streamed_len as usize)
        .min(notice.total_len as usize)
        .min(out.len());
    let read = std::shm_pipe_receive(notice.pipe_id, &mut out[..want])
        .ok()?
        .min(want);
    Some((notice.status, read, notice.total_len))
}

/// Writes text into a line, stopping at the line's end.
fn put(out: &mut [u8], at: &mut usize, text: &[u8]) {
    for byte in text {
        if *at >= out.len() {
            break;
        }
        out[*at] = *byte;
        *at += 1;
    }
}

/// One line of the body.
#[derive(Clone, Copy)]
pub struct Line {
    /// The label in the left column.
    pub label: [u8; LABEL_MAX],
    /// The label's length.
    pub label_len: u8,
    /// The value in the right column, or the whole text of a wide line.
    pub value: [u8; WIDE_MAX],
    /// The value's length.
    ///
    /// WHY a u16 and not a byte: this counts into the same buffer the
    /// transcript measures, and a byte could not name a length past two
    /// hundred and fifty-five -- it would have reported a full wide value as
    /// a sixty-four-byte one and painted the difference as a whole sentence.
    pub value_len: u16,
    /// How the value is coloured.
    pub tone: Tone,
    /// Whether the value runs the full width under the label.
    pub wide: bool,
    /// Where each physical row of a wide line begins.
    ///
    /// WHY the row starts are stored and not computed: a wrap that breaks
    /// mid-word reads as a mistake, and finding the break points needs the
    /// whole string. They are found once, when the line is built.
    ///
    /// WHY a u16 and not a byte: a start is an offset into the value buffer,
    /// and a buffer wide enough to hold the platform's whole sentence has row
    /// starts past the two hundred and fifty-five a byte can name. A byte
    /// would wrap 288 to 32 and paint the third row over text the first row
    /// already carried.
    pub row_starts: [u16; WIDE_ROWS],
    /// How many physical rows the line has.
    pub row_count: u8,
    /// The action this line performs when tapped, or -1.
    pub action: i8,
    /// Whether the value is drawn as a status chip rather than as text.
    ///
    /// WHY a chip and not coloured text: a state that is written in colour
    /// is a state a person has to read, and a state that is written as a
    /// dot and a word is a state a person can find at a glance.
    pub chip: bool,
}

impl Line {
    const fn empty() -> Self {
        Self {
            label: [0; LABEL_MAX],
            label_len: 0,
            value: [0; WIDE_MAX],
            value_len: 0,
            tone: Tone::Plain,
            wide: false,
            row_starts: [0; WIDE_ROWS],
            row_count: 1,
            action: -1,
            chip: false,
        }
    }

    /// How many physical rows the line occupies.
    fn rows(&self) -> usize {
        if self.wide {
            let rows = self.row_count as usize;
            if rows == 0 {
                1
            } else {
                rows
            }
        } else {
            1
        }
    }

    /// The byte range of one physical row of a wide line.
    fn row_range(&self, row: usize) -> (usize, usize) {
        let start = self.row_starts[row] as usize;
        let end = if row + 1 < self.row_count as usize {
            self.row_starts[row + 1] as usize
        } else {
            self.value_len as usize
        };
        (start, end)
    }
}

/// Finds where a wide line's physical rows begin.
///
/// WHY the break is at a space: a row that ends in the middle of a word
/// reads as a typo, and a screen that breaks words looks like it does not
/// know how text works.
///
/// WHY the rows are capped: the line's value buffer holds a fixed number of
/// bytes, and a row that begins past the end of the buffer would be a row
/// of nothing.
fn wrap_rows(value: &[u8], cap_px: u32, max_px: usize) -> ([u16; WIDE_ROWS], u8) {
    let mut starts = [0u16; WIDE_ROWS];
    let mut count = 1usize;
    let mut row_start = 0usize;

    while row_start < value.len() && count < WIDE_ROWS {
        // WHY the row is measured and not counted: the platform's face is
        // proportional, so a row's width depends on the letters in it. A
        // character count wrapped every line at fifty-two characters, which
        // is less than half the column the caption scale can fill, so a
        // sentence that fitted on one row was broken into two and the space
        // beside it was left empty.
        let mut limit = row_start;
        let mut width = 0usize;
        while limit < value.len() {
            // WHY the row is capped in bytes as well as in width: a row
            // longer than the face's key is refused by the face and drawn
            // by the bitmap, and the bitmap draws at a whole cell.
            if limit - row_start >= MAX_RUN_BYTES {
                break;
            }
            let step = measure_cap(&value[limit..limit + 1], cap_px);
            if width + step > max_px {
                break;
            }
            width += step;
            limit += 1;
        }
        if limit >= value.len() {
            // The rest of the text fits on this row, so this is the last one.
            break;
        }
        let mut candidate = limit;
        while candidate > row_start && value[candidate - 1] != b' ' {
            candidate -= 1;
        }
        if candidate <= row_start {
            // WHY the hard break: a row of text with no space in it has
            // nowhere better to go.
            starts[count] = limit as u16;
            count += 1;
            row_start = limit;
            continue;
        }
        // WHY the space is left out of the row and the next row starts after
        // it: a row that ends with a space and a row that begins with one
        // both look like the text has a stray character in it.
        starts[count] = candidate as u16;
        count += 1;
        row_start = candidate;
    }
    (starts, count as u8)
}

/// What a card in the body is: a panel of measurements, or a message.
pub const CARD_PANEL: u8 = 0;
/// A message the platform said: a bubble on the left.
pub const CARD_BUBBLE_THEM: u8 = 1;
/// A message the person said: a bubble on the right.
pub const CARD_BUBBLE_ME: u8 = 2;

/// One card of the body.
#[derive(Clone, Copy)]
pub struct Card {
    /// The card's heading.
    pub title: [u8; TITLE_MAX],
    /// The heading's length.
    pub title_len: u8,
    /// How the heading is coloured.
    pub title_tone: Tone,
    /// The first line of the card in the body's line array.
    pub line_start: u8,
    /// How many lines the card holds.
    pub line_count: u8,
    /// The card's top, in body coordinates.
    pub y: u16,
    /// The card's height.
    pub height: u16,
    /// Whether the card is a panel or a message bubble.
    pub kind: u8,
    /// The bubble's width, in pixels.
    ///
    /// WHY a bubble carries its own width and a panel does not: a panel is
    /// the full content column, and a bubble is as wide as the words in it.
    /// A bubble drawn at the full width is a paragraph in a box, which is
    /// what a message is not.
    pub width: u16,
}

impl Card {
    const fn empty() -> Self {
        Self {
            title: [0; TITLE_MAX],
            title_len: 0,
            title_tone: Tone::Muted,
            line_start: 0,
            line_count: 0,
            y: 0,
            height: 0,
            kind: CARD_PANEL,
            width: 0,
        }
    }
}

/// What a bottom action bar button does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Re-measure the device.
    Refresh,
    /// Ask the platform to install the selected provider's package.
    Install,
    /// Ask the platform to remove the selected provider's package.
    Remove,
    /// Run the self-test.
    RunTest,
    /// Run bounded agent tools.
    RunTools,
    /// Send the prompt to whatever model the device has.
    Send,
    /// Stop/cancel active streaming token generation.
    CancelStream,
    /// Ask the platform to embed the text in the composer.
    ProbeEmbed,
    /// Empty the conversation and the prompt.
    ClearChat,
    /// Go to the conversation.
    OpenChat,
}

/// A bottom action bar button.
#[derive(Clone, Copy)]
pub struct ActionButton {
    /// The button's label.
    pub label: &'static str,
    /// What it does.
    pub action: Action,
    /// Whether it can be pressed.
    pub enabled: bool,
}

// -----------------------------------------------------------------------------
// The app
// -----------------------------------------------------------------------------

/// The agent manager's state.
pub struct AgentManagerApp {
    /// The surface on screen.
    pub surface: Surface,
    /// What was measured about the device.
    pub device: DeviceState,
    /// The capability grants the platform reported.
    pub grants: [CapabilityGrant; CAPABILITY_COUNT],
    /// The search configuration the platform holds.
    pub search: SearchConfiguration,
    /// The self-test's last run.
    pub self_test: SelfTest,
    /// Which provider is selected, by index into the catalogue.
    pub selected: usize,
    /// The last action's result, in the platform's own terms.
    pub action_log: [u8; ACTION_LOG_MAX],
    /// The result's length.
    pub action_log_len: usize,
    /// How the result is coloured.
    pub action_tone: Tone,
    /// The body's scroll position.
    pub scroll: ScrollState,
    /// The body's cards.
    pub cards: [Card; MAX_CARDS],
    /// How many cards the body holds.
    pub card_count: u8,
    /// The body's lines.
    pub lines: [Line; MAX_LINES],
    /// How many lines the body holds.
    pub line_count: u8,
    /// The body's total height, in body coordinates.
    pub content_height: u16,
    /// The conversation and the prompt being written.
    pub chat: ChatSession,
    /// Whether the overflow sheet is open.
    pub sheet_open: bool,
    /// Storage capacity in bytes from the microkernel driver.
    pub storage_bytes: Option<u64>,
    /// The platform's answer about this app's authority to run agent tools.
    pub tool_authority: ToolAuthority,
    /// What the platform said the last time this app asked for that authority.
    pub tool_consent: ConsentOutcome,
    /// Bounded agent tool execution outcomes.
    pub agent_tools: [AgentToolOutcome; 5],
    /// Physical memory diagnostics from microkernel.
    pub diagnostics: RuntimeDiagnostics,
    /// Measured IPC latency in ticks.
    pub ipc_latency_ticks: u64,
    /// The tick by which the platform must have answered the waiting prompt.
    ///
    /// WHY the app holds a deadline rather than a retry count: the answer's cost
    /// is the model's, and a count would punish a slow service for nothing while
    /// a clock tells the person when the app stopped believing one is coming.
    pub answer_deadline: u64,
    /// The tick by which the platform must have reported an install this app asked for.
    ///
    /// WHY it is separate from the prompt's deadline: an install is answered by the
    /// storage service copying a file and the inference service measuring the result,
    /// and the report that comes back is about a file, not an answer to a prompt. A
    /// reader that took it as the model's reply would paint "installed" in the
    /// assistant's bubble.
    pub install_deadline: u64,
    /// The platform band's clock lane, as the RTC answered it.
    ///
    /// WHY it is cached on the app: the read is a syscall and the band is
    /// rasterised per pixel, so a band that asked per pixel would ask
    /// thousands of times a frame. The loop reads it once per tick and hands
    /// the bytes down.
    pub clock: [u8; CLOCK_LEN],
    /// Whether the RTC answered at all. When it did not, the lane stays blank.
    pub clock_ok: bool,
    /// The last embedding the app asked the platform for, and what came back.
    ///
    /// WHY it is held rather than printed at the moment it arrives: the reply
    /// lands whenever the service gets to it, and a surface that only showed it
    /// inside the breath it arrived on would be a surface whose measurement
    /// disappears when the person looks for it. The probe's own state names
    /// whether an answer has been read, so a request that is still standing is
    /// never painted as one that was answered.
    pub probe: EmbedProbe,
}

// WHY at compile time: the app is a stack local in `_start`, and the user
// stack the kernel gives an app is `USER_STACK_SIZE = 256 * 1024`
// (kernel/src/task/user.rs). Widening the transcript to the service's whole
// notice window widens every one of the `MAX_LINES` wide buffers with it, so
// the cost is measured, not assumed: the app's own frame may not eat more
// than half the stack it lives on, and the body's line array -- the term
// that grows with `WIDE_MAX` -- is named beside it so the next widening sees
// what it is paying for. Measured at the window's width: the app is 64.6 KiB
// and the line array 52.9 KiB of the 256 KiB the kernel hands out, the
// deepest chain (`_start` -> event -> `rebuild`/`layout`, and the per-pixel
// painters) adds single-digit kilobytes on top, and `rebuild` clears its
// lines in place so no frame ever carries a second copy of the array.
const USER_STACK_BYTES: usize = 256 * 1024;
const _: () = assert!(
    core::mem::size_of::<AgentManagerApp>() * 2 <= USER_STACK_BYTES,
    "the app's own stack frame outgrew half of the kernel's user stack"
);
const _: () = assert!(
    core::mem::size_of::<Line>() * MAX_LINES * 2 <= USER_STACK_BYTES,
    "the body's line array outgrew half of the kernel's user stack"
);

impl AgentManagerApp {
    /// Builds the app and takes the first measurement.
    pub fn new() -> Self {
        let pref_idx = load_saved_provider().unwrap_or(0);
        let authority = ToolAuthority::probe();
        let mut app = Self {
            surface: Surface::Models,
            device: DeviceState::measure(),
            grants: probe_capability_grants(),
            search: SearchConfiguration::measure(),
            self_test: SelfTest::run(),
            selected: pref_idx,
            action_log: [0; ACTION_LOG_MAX],
            action_log_len: 0,
            action_tone: Tone::Muted,
            scroll: ScrollState::new(0, band_height(), 0),
            cards: [Card::empty(); MAX_CARDS],
            card_count: 0,
            lines: [Line::empty(); MAX_LINES],
            line_count: 0,
            content_height: 0,
            chat: ChatSession::new(),
            sheet_open: false,
            storage_bytes: std::storage_query().ok(),
            tool_authority: authority,
            tool_consent: ConsentOutcome::Refused,
            agent_tools: [
                AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
            ],
            diagnostics: RuntimeDiagnostics::measure(),
            ipc_latency_ticks: 0,
            answer_deadline: 0,
            install_deadline: 0,
            clock: [b' '; CLOCK_LEN],
            clock_ok: false,
            probe: EmbedProbe::new(),
        };
        app.set_action(b"Measurements taken from the platform.", Tone::Muted);
        app.rebuild();
        app
    }

    /// Records the result of the last action.
    pub fn set_action(&mut self, message: &[u8], tone: Tone) {
        let len = message.len().min(ACTION_LOG_MAX);
        self.action_log[..len].copy_from_slice(&message[..len]);
        self.action_log_len = len;
        self.action_tone = tone;
    }

    /// The provider the app has selected.
    pub fn selected_provider(&self) -> &'static dyn Provider {
        let catalogue = providers();
        let index = self.selected.min(catalogue.len().saturating_sub(1));
        catalogue[index]
    }

    /// Re-measures everything the app reports and rebuilds the body.
    pub fn refresh(&mut self) {
        self.device = DeviceState::measure();
        self.storage_bytes = std::storage_query().ok();
        self.grants = probe_capability_grants();
        self.search = SearchConfiguration::measure();
        self.diagnostics = RuntimeDiagnostics::measure();
        let authority = ToolAuthority::probe();
        self.tool_authority = authority;
        self.agent_tools = [
            AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
        ];
        self.set_action(b"Re-measured: device, grants and search configuration.", Tone::Muted);
        self.rebuild();
    }

    /// Runs the bounded agent tools under the platform's authority.
    ///
    /// WHY the app asks the platform before it runs anything: the platform is
    /// the only party that can mint the agent-task capability, and a tool that
    /// ran without it would be the app deciding on its own that it may.
    pub fn run_tools(&mut self) {
        self.tool_consent = ToolAuthority::request();
        let authority = ToolAuthority::probe();
        self.tool_authority = authority;
        self.agent_tools = [
            AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
        ];
        match self.tool_consent {
            ConsentOutcome::Granted => {
                self.set_action(b"Agent tools ran under the platform's grant.", Tone::Good);
            }
            ConsentOutcome::Pending => {
                self.set_action(
                    b"Consent requested: the platform is waiting for your decision.",
                    Tone::Muted,
                );
            }
            ConsentOutcome::Refused => {
                self.set_action(
                    b"Consent refused: the platform holds no agent task capability.",
                    Tone::Bad,
                );
            }
        }
        self.rebuild();
    }

    /// Runs the self-test again and rebuilds the body.
    pub fn run_self_test(&mut self) {
        self.self_test = SelfTest::run();
        let passed = self.self_test.rows.iter().filter(|row| row.passed).count();
        if self.self_test.all_passed {
            self.set_action(b"Self-test finished: every row passed.", Tone::Good);
        } else {
            let mut message = [0u8; 48];
            let mut n = 0;
            for byte in b"Self-test finished: " {
                message[n] = *byte;
                n += 1;
            }
            n += agent_manager::format_u32(passed as u32, &mut message[n..]);
            for byte in b" of 6 rows passed." {
                message[n] = *byte;
                n += 1;
            }
            self.set_action(&message[..n], Tone::Bad);
        }
        self.rebuild();
    }

    /// Asks the platform to install the selected provider's package.
    ///
    /// WHY the platform's own call and not a local flag: the app has no
    /// authority of its own here. The kernel checks the package's signature,
    /// the caller's filesystem-write capability and the consent, and the
    /// answer it gives is the answer the screen shows.
    pub fn install_selected(&mut self) {
        let provider = self.selected_provider();
        if provider.installs_from_device() {
            self.request_model_install();
            return;
        }
        let outcome = transact_package_install(provider, &self.device, std::package_install);
        match outcome {
            PackageInstallOutcome::Installed => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" installed successfully." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Good);
            }
            PackageInstallOutcome::AlreadyInstalled => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" is already installed." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Plain);
            }
            PackageInstallOutcome::RolledBack(code) => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: failed for " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" (rolled back, error " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageInstallOutcome::PermissionDenied => {
                self.set_action(
                    b"INSTALL: permission denied (requires FileSystemWrite capability).",
                    Tone::Unavailable,
                );
            }
            PackageInstallOutcome::NoPackage => {
                self.set_action(
                    b"INSTALL: this provider is not delivered as a package, so there is nothing to install.",
                    Tone::Unavailable,
                );
            }
        }
        self.device = DeviceState::measure();
        self.rebuild();
    }

    /// Asks the inference service to install the model file the device already holds.
    ///
    /// WHY the service and not the filesystem: the model namespace is written by the
    /// storage service, which is the one service the platform grants filesystem write
    /// to, and this app holds no such authority. Asking is the only route that does
    /// not mint a permission.
    ///
    /// WHY the report is not read here: the platform has only just been told, so
    /// anything already queued predates the request. The event loop reads it once the
    /// platform says a message is waiting.
    fn request_model_install(&mut self) {
        self.discard_stale_answers();
        if std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_INSTALL,
            b"INSL",
        )
        .is_err()
        {
            self.set_action(
                b"INSTALL: the inference service would not take the request.",
                Tone::Unavailable,
            );
            self.rebuild();
            return;
        }
        self.install_deadline =
            std::get_ticks().unwrap_or(0).saturating_add(ANSWER_TIMEOUT_TICKS);
        self.set_action(b"Install requested: waiting for the platform's report.", Tone::Muted);
        self.rebuild();
    }

    /// Takes the install report the platform sent, if one has arrived.
    ///
    /// WHY the message type is read before the body: the mailbox also holds the
    /// kernel's copy of every input event dispatched to this window, and a touch read
    /// as an install report would paint a coordinate as the platform's answer about a
    /// file.
    ///
    /// WHY the sentence is shown as it arrived: the service's words are a measurement
    /// of the model namespace, which is the only statement anyone can make about bytes
    /// this app did not copy. A sentence written here would be the app claiming a file
    /// it never saw.
    pub fn step_install_reply(&mut self) -> bool {
        let mut rx = [0u8; 64];
        match std::ipc::receive_any(&mut rx) {
            Err(_) => {
                if std::get_ticks().unwrap_or(0) >= self.install_deadline {
                    self.install_deadline = 0;
                    self.chat
                        .push(ChatRole::Notice, b"The platform sent no install report.");
                    self.set_action(b"No install report received.", Tone::Unavailable);
                    self.device = DeviceState::measure();
                    self.rebuild();
                    return true;
                }
                let _ = std::event_poll(
                    std::EVENT_IPC_MSG | std::EVENT_SHM_READABLE | std::EVENT_TOUCH_INPUT,
                    ANSWER_POLL_TICKS,
                );
                false
            }
            Ok((_, std::ai::MSG_TYPE_INSTALL_CHUNK, len)) => {
                // WHY the notice is copied before the frame is read: the sentence
                // lands in the same buffer the mailbox message was received into, and
                // the channel is only named inside it.
                let mut notice_bytes = [0u8; std::ai::AiStreamNotice::WIRE_LEN];
                let take = len.min(notice_bytes.len());
                notice_bytes[..take].copy_from_slice(&rx[..take]);
                let mut text = [0u8; 256];
                match streamed_install_notice(&notice_bytes, &mut text) {
                    Some((status, read, offered)) => {
                        self.take_install_report(status, &text[..read], offered)
                    }
                    None => {
                        self.install_deadline = 0;
                        self.chat
                            .push(ChatRole::Notice, b"A reply arrived that this app cannot read.");
                        self.set_action(
                            b"Unreadable install report from the inference service.",
                            Tone::Unavailable,
                        );
                        self.rebuild();
                        true
                    }
                }
            }
            Ok((_, std::ai::MSG_TYPE_INSTALL, len)) => {
                let hdr = core::mem::size_of::<std::ai::AiInferenceResponse>();
                if len <= hdr {
                    self.install_deadline = 0;
                    self.chat
                        .push(ChatRole::Notice, b"A reply arrived that this app cannot read.");
                    self.set_action(
                        b"Unreadable install report from the inference service.",
                        Tone::Unavailable,
                    );
                    self.rebuild();
                    return true;
                }
                let resp = unsafe {
                    core::ptr::read_unaligned(rx.as_ptr() as *const std::ai::AiInferenceResponse)
                };
                // WHY the header's length is believed: the mailbox cuts a message at
                // sixty-four bytes and says nothing, so the number the service wrote
                // is the only witness that the sentence it sent was longer than what
                // reached the screen.
                self.take_install_report(resp.status, &rx[hdr..len], resp.output_dim)
            }
            Ok(_) => false,
        }
    }

    /// Paints the platform's install sentence as the platform's own words.
    fn take_install_report(&mut self, status: u32, text: &[u8], offered: u32) -> bool {
        self.install_deadline = 0;
        self.chat.push(ChatRole::Notice, text);
        let arrived = text.len() as u32;
        if arrived < offered {
            let mut line = [0u8; 64];
            let mut n = 0;
            put(&mut line, &mut n, b"Report cut in transit: ");
            n += format_u32(arrived, &mut line[n..]);
            put(&mut line, &mut n, b" of ");
            n += format_u32(offered, &mut line[n..]);
            put(&mut line, &mut n, b" bytes reached the app.");
            self.set_action(&line[..n], Tone::Unavailable);
        } else if status == std::ai::STATUS_SUCCESS {
            self.set_action(b"Install report received.", Tone::Good);
        } else {
            self.set_action(b"The platform refused the install.", Tone::Unavailable);
        }
        self.device = DeviceState::measure();
        self.rebuild();
        true
    }

    /// Asks the platform to remove the selected provider's package.
    pub fn remove_selected(&mut self) {
        let provider = self.selected_provider();
        let outcome = transact_package_remove(
            provider,
            &self.device,
            self.selected,
            std::package_uninstall,
            std::fs_unlink,
        );
        match outcome {
            PackageRemoveOutcome::Uninstalled => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" uninstalled." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Good);
            }
            PackageRemoveOutcome::InUse => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: refused - " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" is currently in use." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::NotFound => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" was not found in registry." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::ModelFileUnlinked => {
                self.set_action(b"REMOVE: /data/models/local_model.gguf unlinked.", Tone::Good);
            }
            PackageRemoveOutcome::ReadOnlyFileSystem(code) => {
                let mut msg = [0u8; 64];
                let mut n = 0;
                for &b in b"REMOVE: /system/models is read-only (code " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::ModelFileFailed(code) => {
                let mut msg = [0u8; 64];
                let mut n = 0;
                for &b in b"REMOVE: unlinking model file failed (code " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::PermissionDenied => {
                self.set_action(
                    b"REMOVE: permission denied (requires FileSystemWrite capability).",
                    Tone::Unavailable,
                );
            }
            PackageRemoveOutcome::NotRemovable => {
                self.set_action(
                    b"REMOVE: this provider is not delivered as a package, so there is nothing to remove.",
                    Tone::Unavailable,
                );
            }
            PackageRemoveOutcome::Failed(code) => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: failed for " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" (error " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
        }
        self.device = DeviceState::measure();
        self.rebuild();
    }

    /// Selects a provider and rebuilds the body.
    pub fn select_provider(&mut self, index: usize) {
        let catalogue = providers();
        if index >= catalogue.len() {
            return;
        }
        self.selected = index;
        save_preferred_provider(index);
        let provider = catalogue[index];
        let mut message = [0u8; ACTION_LOG_MAX];
        let mut n = 0;
        for byte in b"Selected: " {
            message[n] = *byte;
            n += 1;
        }
        let name = provider.display_name().as_bytes();
        let take = name.len().min(ACTION_LOG_MAX - n);
        message[n..n + take].copy_from_slice(&name[..take]);
        n += take;
        self.set_action(&message[..n], Tone::Accent);
        self.rebuild();
    }

    // -------------------------------------------------------------------------
    // Building the body
    // -------------------------------------------------------------------------

    /// The bottom of the body's band on the current surface.
    ///
    /// WHY the band moves: the chat surface carries a composer, and a
    /// composer that scrolls away with the conversation is a composer that
    /// cannot be reached while the answer is being read.
    pub fn body_bottom(&self) -> usize {
        if self.surface == Surface::Chat {
            ACTION_TOP - composer_h()
        } else {
            ACTION_TOP
        }
    }

    /// The height of the body's band on the current surface.
    pub fn body_h(&self) -> usize {
        self.body_bottom().saturating_sub(body_top())
    }

    /// Rebuilds the body for the current surface.
    pub fn rebuild(&mut self) {
        // WHY the lines are cleared in place: `MAX_LINES * size_of::<Line>()`
        // is a body of bytes on the order of the app's own stack frame, and
        // `self.lines = [Line::empty(); MAX_LINES]` hands the compiler a
        // temporary that wide to build on this frame before the move. The
        // app already lives on the entry frame; a second copy of the body
        // on top of it, inside a callee frame, is how a widening silently
        // eats the user stack. One line of scratch at a time, written where
        // it belongs, costs what it costs whether or not the optimiser sees
        // through the temporary.
        for line in self.lines.iter_mut() {
            *line = Line::empty();
        }
        self.cards = [Card::empty(); MAX_CARDS];
        self.line_count = 0;
        self.card_count = 0;

        match self.surface {
            Surface::Models => self.build_models(),
            Surface::Access => self.build_access(),
            Surface::Runtime => self.build_runtime(),
            Surface::Search => self.build_search(),
            Surface::Test => self.build_test(),
            Surface::Chat => self.build_chat(),
        }

        self.layout();
    }

    /// Starts a card.
    fn begin_card(&mut self, title: &[u8], tone: Tone) {
        let index = self.card_count as usize;
        if index >= MAX_CARDS {
            return;
        }
        let mut card = Card::empty();
        let len = title.len().min(TITLE_MAX);
        card.title[..len].copy_from_slice(&title[..len]);
        card.title_len = len as u8;
        card.title_tone = tone;
        card.line_start = self.line_count;
        card.kind = CARD_PANEL;
        self.cards[index] = card;
        self.card_count += 1;
    }

    /// Starts a message bubble.
    ///
    /// WHY a bubble has no heading: the speaker is shown by which side the
    /// bubble is on, and a heading above it would say the same thing twice.
    fn begin_bubble(&mut self, kind: u8) {
        let index = self.card_count as usize;
        if index >= MAX_CARDS {
            return;
        }
        let mut card = Card::empty();
        card.line_start = self.line_count;
        card.kind = kind;
        self.cards[index] = card;
        self.card_count += 1;
    }

    /// Appends a line with a label and a value.
    fn add_line(&mut self, label: &[u8], value: &[u8], tone: Tone) {
        self.push_line(label, value, tone, false, -1, false);
    }

    /// Appends a line whose value is a status chip.
    fn add_chip(&mut self, label: &[u8], value: &[u8], tone: Tone) {
        self.push_line(label, value, tone, false, -1, true);
    }

    /// Appends a line that can be tapped to select a provider and whose
    /// value is a status chip.
    fn add_selectable_chip(&mut self, label: &[u8], value: &[u8], tone: Tone, action: i8) {
        self.push_line(label, value, tone, false, action, true);
    }

    /// Appends a line that runs the full width of the card.
    fn add_wide(&mut self, value: &[u8], tone: Tone) {
        self.push_line(b"", value, tone, true, -1, false);
    }

    fn push_line(
        &mut self,
        label: &[u8],
        value: &[u8],
        tone: Tone,
        wide: bool,
        action: i8,
        chip: bool,
    ) {
        let index = self.line_count as usize;
        if index >= MAX_LINES {
            return;
        }
        let mut line = Line::empty();
        let label_len = label.len().min(LABEL_MAX);
        line.label[..label_len].copy_from_slice(&label[..label_len]);
        if label.len() > LABEL_MAX {
            // WHY an ellipsis: a cut that looks like a whole label is a label
            // that changed the meaning. This one carried the expression the
            // number beside it came from, and half an expression reads as a
            // different expression. The mark says the text continues.
            line.label[LABEL_MAX - 3..LABEL_MAX].copy_from_slice(b"...");
        }
        line.label_len = label_len as u8;
        let value_len = value.len();
        let kept = value_len.min(WIDE_MAX);
        line.value[..kept].copy_from_slice(&value[..kept]);
        if value_len > WIDE_MAX {
            // WHY an ellipsis: a wide line that does not fit is cut, and a
            // cut that looks like a whole sentence is a sentence that
            // changed the meaning. The mark says the text continues.
            line.value[WIDE_MAX - 3..WIDE_MAX].copy_from_slice(b"...");
            line.value_len = WIDE_MAX as u16;
        } else {
            line.value_len = kept as u16;
        }
        line.tone = tone;
        line.wide = wide;
        if wide {
            let (starts, count) = wrap_rows(&line.value[..line.value_len as usize], cap_caption(), WIDE_W);
            line.row_starts = starts;
            line.row_count = count;
        }
        line.action = action;
        line.chip = chip;
        self.lines[index] = line;
        self.line_count += 1;

        // WHY the card's own count is bumped here: a line belongs to the card
        // that is open, and counting them anywhere else would leave a card
        // that knows its first line but not how many it has.
        if self.card_count > 0 {
            let card_index = self.card_count as usize - 1;
            self.cards[card_index].line_count += 1;
        }
    }

    /// Places the cards and measures the body's height.
    fn layout(&mut self) {
        let mut y = 0usize;
        for index in 0..self.card_count as usize {
            let card = self.cards[index];
            let mut height;
            let mut width = CONTENT_W;

            if card.kind == CARD_PANEL {
                height = lines_top();
                for line_index in 0..card.line_count as usize {
                    let line = self.lines[card.line_start as usize + line_index];
                    height += line.rows() * row_h();
                }
                height += CARD_PAD_BOTTOM;
            } else {
                // WHY a bubble is measured by its longest row: a bubble that
                // is the full width of the column is a paragraph in a box,
                // and a bubble that is wider than its words is a bubble with
                // a hole in it. The width is the words plus the padding
                // they sit inside.
                let mut widest = 0usize;
                let mut rows_total = 0usize;
                for line_index in 0..card.line_count as usize {
                    let line = self.lines[card.line_start as usize + line_index];
                    rows_total += line.rows();
                    for row in 0..line.rows() {
                        let (start, end) = line.row_range(row);
                        widest = widest.max(measure_cap(&line.value[start..end], cap_caption()));
                    }
                }
                width = (widest + 2 * BUBBLE_PAD).min(CONTENT_W);
                height = 2 * BUBBLE_PAD + rows_total * row_h();
            }

            self.cards[index].y = y as u16;
            self.cards[index].height = height as u16;
            self.cards[index].width = width as u16;
            y += height + CARD_GAP;
        }
        // WHY the band under the last card: the floating button covers the
        // bottom-right of the body, and a value that sits under it is a
        // value a person cannot read. The band is empty space the last card
        // can be scrolled past, so the button never sits on a value.
        //
        // WHY the chat surface has no band: it has no floating button, and a
        // composer that scrolls further than it needs to is a composer that
        // moves under the thumb for no reason.
        let reserved = if self.surface == Surface::Chat {
            0
        } else {
            FAB_R * 2 + FAB_MARGIN
        };
        let content = y.saturating_sub(CARD_GAP) + reserved;
        self.content_height = content as u16;
        self.scroll = ScrollState::new(content, self.body_h(), self.scroll.offset);
    }

    // -------------------------------------------------------------------------
    // The surfaces
    // -------------------------------------------------------------------------

    /// Builds the model management surface.
    fn build_models(&mut self) {
        let models = self.device.models;
        self.begin_card(b"Device model state", Tone::Plain);
        self.add_line(b"Model directory", MODEL_DIRECTORY_LABEL, Tone::Plain);
        self.add_chip(
            b"Directory",
            models.status.as_str().as_bytes(),
            model_tone(models.status),
        );
        let mut count = [0u8; 8];
        let n = format_u32(models.entries as u32, &mut count);
        self.add_line(b"Entries", &count[..n], Tone::Plain);
        if let Some(bytes) = self.storage_bytes {
            let mut storage_buf = [0u8; 24];
            let gb_int = bytes / (1024 * 1024 * 1024);
            let gb_frac = (bytes % (1024 * 1024 * 1024)) * 100 / (1024 * 1024 * 1024);
            let mut sn = format_u32(gb_int as u32, &mut storage_buf);
            storage_buf[sn] = b'.';
            sn += 1;
            if gb_frac < 10 {
                storage_buf[sn] = b'0';
                sn += 1;
            }
            let fn_len = format_u32(gb_frac as u32, &mut storage_buf[sn..]);
            sn += fn_len;
            for &b in b" GB" {
                storage_buf[sn] = b;
                sn += 1;
            }
            self.add_line(b"Disk capacity", &storage_buf[..sn], Tone::Plain);
            let mut mounted_buf = [0u8; 32];
            let mut mn = 0;
            for &b in &storage_buf[..sn] {
                mounted_buf[mn] = b;
                mn += 1;
            }
            for &b in b" MOUNTED" {
                mounted_buf[mn] = b;
                mn += 1;
            }
            self.add_chip(b"Storage link", &mounted_buf[..mn], Tone::Good);
        } else {
            self.add_line(b"Disk capacity", b"Unavailable", Tone::Muted);
            self.add_chip(b"Storage link", b"NOT LINKED", Tone::Bad);
        }
        self.add_chip(
            b"Embedding weights",
            if models.has_embedding_weights {
                b"PRESENT"
            } else {
                b"ABSENT"
            },
            if models.has_embedding_weights {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        self.add_chip(
            b"Generative weights",
            if models.has_generative_weights {
                b"PRESENT"
            } else {
                b"ABSENT"
            },
            if models.has_generative_weights {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        let embed_stage = models.embedding_stage(&self.device.services);
        self.add_chip(
            b"Embedding stage",
            embed_stage.map(|s| s.as_str().as_bytes()).unwrap_or(b"ABSENT"),
            if embed_stage.is_some() { Tone::Good } else { Tone::Muted },
        );
        if models.embedding_size_kb > 0 {
            let mut sbuf = [0u8; 24];
            let mut sn = format_u32(models.embedding_size_kb as u32, &mut sbuf);
            for &b in b" KB" { sbuf[sn] = b; sn += 1; }
            self.add_line(b"Embedding size", &sbuf[..sn], Tone::Plain);
        }

        let gen_stage = models.generative_stage(&self.device.services);
        self.add_chip(
            b"Generative stage",
            gen_stage.map(|s| s.as_str().as_bytes()).unwrap_or(b"ABSENT"),
            if gen_stage.is_some() { Tone::Good } else { Tone::Muted },
        );
        if models.generative_size_kb > 0 {
            let mut sbuf = [0u8; 24];
            let mut sn = format_u32(models.generative_size_kb as u32, &mut sbuf);
            for &b in b" KB" { sbuf[sn] = b; sn += 1; }
            self.add_line(b"Generative size", &sbuf[..sn], Tone::Plain);
        }
        if models.status == ModelProbeStatus::Absent {
            self.add_wide(
                b"The platform has no model directory on this device, so the device holds no model weights.",
                Tone::Muted,
            );
        } else if models.status == ModelProbeStatus::Refused {
            self.add_wide(
                b"The platform refused the listing, so this app cannot say what the device holds.",
                Tone::Unavailable,
            );
        } else if models.status == ModelProbeStatus::NoAuthority {
            self.add_wide(
                b"This app holds no filesystem-read capability, so it cannot list the model directory.",
                Tone::Unavailable,
            );
        }

        self.begin_card(b"Provider catalogue", Tone::Accent);
        for (index, provider) in providers().iter().enumerate() {
            let state = provider.install_state(&self.device);
            self.add_selectable_chip(
                provider.display_name().as_bytes(),
                state.as_str().as_bytes(),
                install_tone(state),
                index as i8,
            );
            self.add_wide(provider.capabilities().role().as_bytes(), Tone::Muted);
        }
        self.add_wide(
            b"The install state is a measurement of this device, not a vendor's plan.",
            Tone::Muted,
        );

        let selected = self.selected_provider();
        self.begin_card(b"Selected provider", Tone::Plain);
        self.add_line(b"Provider", selected.display_name().as_bytes(), Tone::Accent);
        self.add_line(b"Identifier", selected.id().as_bytes(), Tone::Plain);
        let state = selected.install_state(&self.device);
        self.add_chip(b"Install state", state.as_str().as_bytes(), install_tone(state));
        let binding = selected.runtime_binding(&self.device);
        self.add_chip(
            b"Runtime",
            binding.as_str().as_bytes(),
            if binding.is_bound() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        if let RuntimeBinding::Unbound { reason } = binding {
            self.add_wide(reason.as_bytes(), Tone::Muted);
        }
        self.add_line(
            b"Package",
            selected.package_id().unwrap_or("none").as_bytes(),
            Tone::Plain,
        );
        if let Some(version) = selected.package_version(&self.device) {
            let mut ver_buf = [0u8; 32];
            let mut n = 0;
            n += format_u32(version.0 as u32, &mut ver_buf[n..]);
            ver_buf[n] = b'.'; n += 1;
            n += format_u32(version.1 as u32, &mut ver_buf[n..]);
            ver_buf[n] = b'.'; n += 1;
            n += format_u32(version.2 as u32, &mut ver_buf[n..]);
            self.add_line(b"Pkg version", &ver_buf[..n], Tone::Plain);
        }
        if let Some(pkg_state) = selected.package_lifecycle_state(&self.device) {
            self.add_chip(
                b"Pkg state",
                pkg_state.as_str().as_bytes(),
                if pkg_state == RegistryPackageState::Active {
                    Tone::Good
                } else {
                    Tone::Unavailable
                },
            );
        }

    }

    /// Builds the permissions surface.
    fn build_access(&mut self) {
        // WHY the grants are copied out: the builder mutates the body while
        // it walks the list, and the list belongs to the app.
        let grants = self.grants;
        self.begin_card(b"Capability grants", Tone::Plain);
        let granted = granted_capability_count(&grants);
        let mut summary = [0u8; 24];
        let mut n = 0;
        n += format_u32(granted as u32, &mut summary[n..]);
        for byte in b" of " {
            summary[n] = *byte;
            n += 1;
        }
        n += format_u32(CAPABILITY_COUNT as u32, &mut summary[n..]);
        for byte in b" granted" {
            summary[n] = *byte;
            n += 1;
        }
        self.add_chip(b"Granted", &summary[..n], Tone::Plain);
        for grant in grants.iter() {
            self.add_chip(
                grant.name.as_bytes(),
                if grant.granted {
                    b"GRANTED"
                } else {
                    b"NOT GRANTED"
                },
                if grant.granted {
                    Tone::Good
                } else {
                    Tone::Bad
                },
            );
        }

        self.begin_card(b"What the grants allow", Tone::Plain);
        let mut any = false;
        for grant in grants.iter() {
            if grant.granted {
                self.add_wide(grant.allows.as_bytes(), Tone::Plain);
                any = true;
            }
        }
        if !any {
            self.add_wide(
                b"The platform granted this app no capability at all.",
                Tone::Unavailable,
            );
        }
        self.add_wide(
            b"GRANTED means the platform's capability table holds that type for this app. It is asked of the kernel, not declared here.",
            Tone::Muted,
        );

        self.begin_card(b"What is not granted", Tone::Plain);
        let mut any = false;
        for grant in grants.iter() {
            if !grant.granted {
                self.add_wide(grant.allows.as_bytes(), Tone::Muted);
                any = true;
            }
        }
        if !any {
            self.add_wide(b"Nothing is missing: every capability asked about was granted.", Tone::Muted);
        }
    }

    /// Builds the runtime status surface.
    fn build_runtime(&mut self) {
        let services = self.device.services;
        self.begin_card(b"Service registry", Tone::Plain);
        let mut count = [0u8; 24];
        let mut n = 0;
        n += format_u32(services.count as u32, &mut count[n..]);
        for byte in b" service(s)" {
            count[n] = *byte;
            n += 1;
        }
        self.add_line(b"Registered", &count[..n], Tone::Plain);
        self.add_line(
            b"Registry query",
            services.status.as_str().as_bytes(),
            match services.status {
                ProbeStatus::Listed => Tone::Good,
                ProbeStatus::Refused => Tone::Unavailable,
            },
        );
        for entry in services.entries.iter().take(services.count as usize) {
            self.add_line(
                service_name(entry.id).as_bytes(),
                entry.state.as_str().as_bytes(),
                service_tone(entry.state),
            );
        }
        if services.count == 0 && services.status == ProbeStatus::Listed {
            self.add_wide(b"The registry answered and listed no services.", Tone::Muted);
        }

        self.begin_card(b"Inference runtime", Tone::Accent);
        let binding = services.inference_runtime_binding();
        self.add_line(
            b"Runtime",
            binding.as_str().as_bytes(),
            if binding.is_bound() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        if let RuntimeBinding::Unbound { reason } = binding {
            self.add_wide(reason.as_bytes(), Tone::Muted);
        }
        let mut highest = [0u8; 16];
        let n = format_u32(services.highest_id as u32, &mut highest);
        self.add_line(b"Highest service id", &highest[..n], Tone::Plain);
        self.add_line(b"Service ABI range", b"1..=11", Tone::Plain);
        self.add_wide(
            b"The launcher-to-service path, the authorisation and the retrieval are not wired up on this platform, so no system-wide integration is claimed.",
            Tone::Muted,
        );

        self.begin_card(b"System telemetry & diagnostics", Tone::Plain);
        if self.diagnostics.has_ram_telemetry {
            let mut buf = [0u8; 24];
            let mut n = format_u32(self.diagnostics.ram_total_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM total", &buf[..n], Tone::Plain);

            let mut n = format_u32(self.diagnostics.ram_used_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM used", &buf[..n], Tone::Plain);

            let mut n = format_u32(self.diagnostics.ram_free_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM free", &buf[..n], Tone::Plain);
        } else {
            self.add_line(b"Physical RAM", b"Unavailable", Tone::Muted);
        }

        if self.ipc_latency_ticks > 0 {
            let mut lbuf = [0u8; 24];
            let mut ln = format_u32(self.ipc_latency_ticks as u32, &mut lbuf);
            for &b in b" ticks" { lbuf[ln] = b; ln += 1; }
            self.add_line(b"IPC latency", &lbuf[..ln], Tone::Plain);
        } else if self.device.services.contains_inference_runtime() {
            self.add_line(b"IPC latency", b"Active", Tone::Good);
        } else {
            self.add_line(b"IPC latency", b"Unbound: no target", Tone::Muted);
        }

        self.add_line(
            b"Generation stream",
            if self.chat.is_streaming { b"Streaming active" } else { b"Idle" },
            if self.chat.is_streaming { Tone::Accent } else { Tone::Plain },
        );
    }

    /// Builds the search configuration surface.
    fn build_search(&mut self) {
        let search = self.search;
        self.begin_card(b"Retrieval settings", Tone::Plain);
        let held = search.dimension.is_some();
        let dimension = search.dimension.unwrap_or(MATRYOSHKA_DEFAULT_DIM);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_u32(dimension as u32, &mut value);
        for byte in if held {
            &b" (platform)"[..]
        } else {
            &b" (default)"[..]
        } {
            value[n] = *byte;
            n += 1;
        }
        self.add_line(b"Vector dimension", &value[..n], Tone::Plain);

        let held = search.threshold_q15.is_some();
        let threshold = search.threshold_q15.unwrap_or(SIMILARITY_DEFAULT_Q15);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_threshold(threshold, &mut value);
        for byte in if held {
            &b" platform"[..]
        } else {
            &b" default"[..]
        } {
            if n < value.len() {
                value[n] = *byte;
                n += 1;
            }
        }
        self.add_line(b"Similarity cutoff", &value[..n], Tone::Plain);

        let held = search.max_results.is_some();
        let max_results = search.max_results.unwrap_or(MAX_RESULTS_DEFAULT);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_u32(max_results as u32, &mut value);
        for byte in if held {
            &b" (platform)"[..]
        } else {
            &b" (default)"[..]
        } {
            value[n] = *byte;
            n += 1;
        }
        self.add_line(b"Max results", &value[..n], Tone::Plain);

        self.add_line(
            b"Filesystem scan",
            if search.filesystem_scan {
                b"ON"
            } else {
                b"OFF"
            },
            if search.filesystem_scan {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        self.add_line(
            b"Settings are",
            if search.writable {
                b"writable"
            } else {
                b"read-only here"
            },
            if search.writable {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        if search.dimension.is_none() {
            self.add_wide(
                b"The platform's settings store holds no search.* key, so the values above are the agent manager's own defaults.",
                Tone::Muted,
            );
        }

        self.begin_card(b"Search sources", Tone::Plain);
        for source in search.sources.iter() {
            let mut value = [0u8; VALUE_MAX];
            let n = if source.status == ProbeStatus::Listed {
                format_u32(source.count as u32, &mut value)
            } else {
                let text = b"REFUSED";
                value[..text.len()].copy_from_slice(text);
                text.len()
            };
            self.add_line(
                source.name.as_bytes(),
                &value[..n],
                match source.status {
                    ProbeStatus::Listed => Tone::Plain,
                    ProbeStatus::Refused => Tone::Unavailable,
                },
            );
        }
        self.add_wide(
            b"A refused source is a capability the platform withheld, not an empty one.",
            Tone::Muted,
        );

        self.begin_card(b"Content layer", Tone::Accent);
        self.add_wide(
            b"The document index and the intent registry are the platform's own. The counts above are the platform's answers.",
            Tone::Muted,
        );

        self.begin_card(b"Document retrieval", Tone::Plain);
        let _ = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_SEARCH,
            b"storage",
        );
        let mut doc_buf = [0u8; 1024];
        let mut shown = 0;
        // WHY the reply is taken by message type: the mailbox also holds the
        // kernel's copy of every input event dispatched to this window, and the
        // first message in the queue is not necessarily the one this card asked
        // for. A touch read as a search reply paints a coordinate as a document.
        let mut reply_len = 0usize;
        for _ in 0..MAILBOX_DRAIN {
            match std::ipc::receive_any(&mut doc_buf) {
                Ok((_, std::ai::MSG_TYPE_SEARCH, len)) => {
                    reply_len = len;
                    break;
                }
                Ok((_, std::ai::MSG_TYPE_SEARCH_CHUNK, len)) => {
                    // WHY the notice is copied before the frame is read: the
                    // frame lands in the same buffer the mailbox message was
                    // received into, and the channel is only named inside it.
                    let mut notice_bytes = [0u8; std::ai::AiStreamNotice::WIRE_LEN];
                    let take = len.min(notice_bytes.len());
                    notice_bytes[..take].copy_from_slice(&doc_buf[..take]);
                    reply_len = streamed_search_frame(&notice_bytes, &mut doc_buf);
                    break;
                }
                Ok((_, std::ipc::MSG_INPUT_EVENT, _))
                | Ok((_, std::ipc::MSG_WINDOW_FOCUSED, _)) => continue,
                Ok((_, std::ai::MSG_TYPE_EMBED, len)) => {
                    // WHY the probe is read here and the drain carries on: this
                    // card is built while an embedding reply may be the next
                    // message in the queue, and a drain that stopped at a message
                    // of another type would lose the reply and the search answer
                    // with it. Reading it here is what keeps the probe's answer
                    // from vanishing between the platform sending it and the
                    // screen being built.
                    self.take_embedding_reply(&doc_buf[..len]);
                    continue;
                }
                _ => break,
            }
        }
        let n = reply_len;
        if n > 0 {
            let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
            let hit_hdr_len = core::mem::size_of::<std::ai::AiSearchHit>();
            if n >= resp_hdr_len + hit_hdr_len {
                let resp = unsafe {
                    core::ptr::read_unaligned(doc_buf.as_ptr() as *const std::ai::AiInferenceResponse)
                };
                if resp.status == std::ai::STATUS_SUCCESS && resp.output_dim > 0 {
                    let mut cursor = resp_hdr_len;
                    let hits_count = resp.output_dim as usize;
                    for _ in 0..hits_count {
                        if cursor + hit_hdr_len > n || shown >= 3 {
                            break;
                        }
                        let hit = unsafe {
                            core::ptr::read_unaligned(doc_buf.as_ptr().add(cursor) as *const std::ai::AiSearchHit)
                        };
                        cursor += hit_hdr_len;
                        let path_len = hit.path_len as usize;
                        let snip_len = hit.snippet_len as usize;
                        if cursor + path_len + snip_len > n {
                            break;
                        }
                        let path_bytes = &doc_buf[cursor..cursor + path_len];
                        cursor += path_len;
                        let snippet_bytes = &doc_buf[cursor..cursor + snip_len];
                        cursor += snip_len;

                        let path_or_snippet = if !path_bytes.is_empty() {
                            path_bytes
                        } else {
                            snippet_bytes
                        };
                        self.add_line(b"Ranked result", path_or_snippet, Tone::Plain);
                        self.add_chip(
                            b"Attribution",
                            if hit.is_neural != 0 {
                                b"[Neural RAG]"
                            } else {
                                b"[Lexical RAG]"
                            },
                            Tone::Good,
                        );
                        shown += 1;
                    }
                }
            }
        }
        if shown == 0 {
            let mut list_buf = [0u8; 512];
            match std::fs_list("/storage", &mut list_buf) {
                Ok(n) if n > 0 => {
                    let payload = &list_buf[..n.min(list_buf.len())];
                    for line in payload.split(|&b| b == b'\n') {
                        if line.is_empty() || shown >= 3 {
                            continue;
                        }
                        let mut parts = line.split(|&b| b == b'|');
                        if let Some(name) = parts.next() {
                            if !name.is_empty() {
                                self.add_line(b"Ranked result", name, Tone::Plain);
                                self.add_chip(b"Attribution", b"[Lexical RAG]", Tone::Good);
                                shown += 1;
                            }
                        }
                    }
                    if shown == 0 {
                        self.add_wide(b"Zero indexed documents reported by content service.", Tone::Muted);
                    }
                }
                _ => {
                    self.add_wide(b"Zero indexed documents reported by content service.", Tone::Muted);
                }
            }
        }

        // WHY the probe is the card this surface ends on: everything above is what
        // the platform says its retrieval is configured to do, and this is the one
        // answer the platform actually sent to this app.
        self.add_embedding_probe_card();
    }

    /// Paints the embedding probe: what was sent, and only what the reply carried.
    ///
    /// WHY every line here is read off the probe's own fields: the platform answers
    /// an embedding request from one of three places -- the installed file's
    /// tensors, its labelled lexical projection, or a sentence that says why
    /// neither ran -- and the app cannot tell them apart from anything but the
    /// status the reply stamped itself. A card that showed a vector for a refusal,
    /// or the baseline's arithmetic under the installed model's name, would be this
    /// app's invention rather than a reading of the bytes.
    ///
    /// WHY there is no length figure of the app's own: the reply's header is
    /// `status`, `output_dim`, `latency_micros` and `output_offset`
    /// (`interos_std::ai::AiInferenceResponse`), and none of them is a norm, so the
    /// screen names the components that arrived and the count the reply offered and
    /// leaves the arithmetic to whoever reads them.
    fn add_embedding_probe_card(&mut self) {
        // WHY the probe is copied out: the builder mutates the body while it walks
        // the reply, and the reply belongs to the app.
        let probe = self.probe;
        self.begin_card(b"Embedding probe", Tone::Plain);
        match probe.state {
            ProbeState::Idle => {
                self.add_wide(
                    b"Nothing asked yet. EMBED TEXT sends the text typed in the chat's composer.",
                    Tone::Muted,
                );
                return;
            }
            ProbeState::Awaiting => {
                self.add_wide(
                    b"Embed request sent: the app has read no reply from the platform yet.",
                    Tone::Muted,
                );
            }
            ProbeState::NoReply => {
                self.add_wide(
                    b"The inference service sent no answer. The app's own wait ran out.",
                    Tone::Unavailable,
                );
            }
            ProbeState::Unreadable => {
                self.add_wide(b"A reply arrived that this app cannot read.", Tone::Unavailable);
            }
            ProbeState::Answered => {}
        }

        // WHAT WAS ASKED, as far as the request could carry it.
        if probe.text_len > 0 {
            let mut value = [0u8; VALUE_MAX];
            let sent = probe.sent_text();
            let take = sent.len().min(VALUE_MAX);
            value[..take].copy_from_slice(&sent[..take]);
            self.add_line(b"Sent text", &value[..take], Tone::Plain);
            if probe.typed_bytes as usize > take {
                // WHY the file's own wording for a cut: this is the sentence
                // `close_answer` and `take_install_report` already use when a
                // platform message reaches the app shorter than it was sent, and
                // the composer held more text than one mailbox message carries.
                let mut line = [0u8; 64];
                let mut n = 0;
                put(&mut line, &mut n, b"Cut in transit: ");
                n += format_u32(take as u32, &mut line[n..]);
                put(&mut line, &mut n, b" of ");
                n += format_u32(probe.typed_bytes as u32, &mut line[n..]);
                put(&mut line, &mut n, b" typed bytes reached the request.");
                self.add_wide(&line[..n], Tone::Unavailable);
            }
        }

        if probe.state == ProbeState::Answered {
            if probe.reply_is_vector {
                // WHY the chip is the status's word and not the bytes': the reply
                // says which arithmetic ran, and only that. The app's two
                // attributions are the ones the search card already uses.
                if let Some(attribution) = embed_attribution(probe.status) {
                    self.add_chip(b"Attribution", attribution, Tone::Good);
                }
                self.add_wide(
                    if probe.status == std::ai::STATUS_BASELINE_LEXICAL {
                        b"The reply stamped the lexical baseline: a projection of the prompt's bytes, with no tensor of any model file read to make it."
                    } else {
                        b"The reply stamped the service's success code, which on this request means the installed file's tensors were read and stepped."
                    },
                    Tone::Muted,
                );
                let mut value = [0u8; VALUE_MAX];
                let n = format_u32(probe.reply_dim, &mut value);
                self.add_line(b"Reply dimension", &value[..n], Tone::Plain);
                let mut line = [0u8; 64];
                let (written, shown) = probe.leading_values(&mut line);
                if shown > 0 {
                    let mut head = [0u8; VALUE_MAX];
                    let mut k = 0;
                    k += format_u32(shown as u32, &mut head[k..]);
                    for byte in b" of " {
                        head[k] = *byte;
                        k += 1;
                    }
                    k += format_u32(probe.values_arrived() as u32, &mut head[k..]);
                    for byte in b" arrived" {
                        head[k] = *byte;
                        k += 1;
                    }
                    self.add_chip(b"Leading values", &line[..written], Tone::Plain);
                    self.add_line(b"Values listed", &head[..k], Tone::Muted);
                }
                if probe.values_arrived() < probe.offered as usize {
                    // WHY both numbers: the mailbox cut the reply at
                    // MAILBOX_PAYLOAD_MAX without a word, so the header is the only
                    // witness that the vector went on past what the app was handed.
                    let mut line = [0u8; 64];
                    let mut n = 0;
                    put(&mut line, &mut n, b"Cut in transit: ");
                    n += format_u32(probe.values_arrived() as u32, &mut line[n..]);
                    put(&mut line, &mut n, b" of ");
                    n += format_u32(probe.offered, &mut line[n..]);
                    put(&mut line, &mut n, b" bytes reached the app.");
                    self.add_wide(&line[..n], Tone::Unavailable);
                }
            } else {
                // WHY the words come out as the platform wrote them: a refusal
                // names the branch that did not run, and that sentence is the
                // observation -- the same thing `send_prompt` does with
                // `availability.reason()`. Nothing here is attributed to a model.
                //
                // WHY the buffer is the notice's width and not the mailbox tail's:
                // the service streams the sentence whole when a channel is free and
                // sends it inline when one is not, and both routes land in
                // `notice_words`. Sizing this to `PROBE_PAYLOAD_MAX` would re-cut at
                // forty bytes the refusal the channel was opened to carry.
                let mut words = [0u8; EMBED_NOTICE_TEXT_MAX];
                let arrived = probe.notice_words(&mut words);
                self.add_wide(&words[..arrived], Tone::Unavailable);
                let mut value = [0u8; VALUE_MAX];
                let mut n = format_u32(probe.offered, &mut value);
                for byte in b" bytes" {
                    if n < value.len() {
                        value[n] = *byte;
                        n += 1;
                    }
                }
                // WHY the count is called a sentence's length and not a dimension:
                // `emit_response_as` puts the byte count of its own sentence in the
                // field a vector reply puts its dimension in.
                self.add_line(b"Reply sentence", &value[..n], Tone::Muted);
                if arrived < probe.offered as usize {
                    let mut line = [0u8; 64];
                    let mut n = 0;
                    put(&mut line, &mut n, b"Cut in transit: ");
                    n += format_u32(arrived as u32, &mut line[n..]);
                    put(&mut line, &mut n, b" of ");
                    n += format_u32(probe.offered, &mut line[n..]);
                    put(&mut line, &mut n, b" bytes reached the app.");
                    self.add_wide(&line[..n], Tone::Unavailable);
                }
            }
        }

        // THE INTERVALS THE APP MEASURED, in ticks of the platform's own clock.
        if probe.state != ProbeState::Idle {
            let mut value = [0u8; VALUE_MAX];
            let n = format_u32(probe.send_ticks.min(u32::MAX as u64) as u32, &mut value);
            self.add_line(b"Send call, ticks", &value[..n], Tone::Plain);
            // WHY the second interval waits for an answer: the span from the
            // request to the reply only exists once a reply -- or the app's own
            // wait -- has ended, and a zero read out here would look like a
            // measurement of an instant rather than of nothing at all.
            if probe.state != ProbeState::Awaiting {
                let mut value = [0u8; VALUE_MAX];
                let n = format_u32(probe.reply_ticks.min(u32::MAX as u64) as u32, &mut value);
                self.add_line(b"Ask to answer, ticks", &value[..n], Tone::Plain);
            }
        }

        // WHY the reply's own latency field is not shown: the service fills it with
        // zero on every sentence reply and with a tick difference multiplied up on a
        // vector reply, and neither is a measurement this app took or can check.
        self.add_wide(
            b"Numbers above are the reply's own fields and this app's own clock.",
            Tone::Muted,
        );
    }

    /// Builds the chat surface.
    ///
    /// WHY the service state comes first: the conversation is the
    /// feature, but the answer to "why is there no reply" is the service
    /// state, and somebody about to type deserves to know before they
    /// type rather than after.
    fn build_chat(&mut self) {
        let availability = resolve_inference_service(&self.device.services);
        self.begin_card(b"Model service", Tone::Plain);
        self.add_chip(
            b"State",
            if availability.is_available() {
                b"running"
            } else {
                b"not available"
            },
            if availability.is_available() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        self.add_wide(availability.reason().as_bytes(), Tone::Muted);

        if self.chat.is_empty() {
            // WHY an empty conversation still says something: a screen that
            // is blank tells the reader nothing, and the honest thing to
            // say is what the surface is for.
            self.begin_bubble(CARD_BUBBLE_THEM);
            self.add_wide(
                b"No messages yet. Type a prompt below and press the send button.",
                Tone::Plain,
            );
        } else {
            for position in 0..self.chat.len() {
                // WHY the message is copied out: the builder mutates the
                // body while it walks the transcript, and the transcript
                // belongs to the app.
                let Some(message) = self.chat.message_at(position).copied() else {
                    break;
                };
                let role = message.role;
                let len = message.text_len as usize;
                // WHY the speaker is shown by the side of the bubble: a
                // label above every message repeats what the layout already
                // says, and the design the showcases describe puts the
                // person on the right and the platform on the left.
                let kind = match role {
                    ChatRole::User => CARD_BUBBLE_ME,
                    _ => CARD_BUBBLE_THEM,
                };
                self.begin_bubble(kind);
                self.add_wide(&message.text[..len], chat_role_ink(role));
            }
        }
    }

    /// Sends the prompt to whatever model the device has, and waits for an answer.
    ///
    /// WHY the prompt is recorded before the answer is sought: the
    /// exchange is the user's, and it belongs in the transcript whether
    /// or not a model was there to answer it. A conversation that
    /// silently swallowed the prompt would leave the user unable to say
    /// what they had asked.
    pub fn send_prompt(&mut self) {
        if self.chat.prompt_is_empty() {
            self.set_action(b"Nothing to send.", Tone::Muted);
            return;
        }
        let mut prompt = [0u8; CHAT_PROMPT_MAX];
        let len = self.chat.prompt_text().len().min(CHAT_PROMPT_MAX);
        prompt[..len].copy_from_slice(&self.chat.prompt_text()[..len]);
        self.chat.push(ChatRole::User, &prompt[..len]);
        self.chat.clear_prompt();

        let availability = resolve_inference_service(&self.device.services);
        if !availability.is_available() {
            // WHY the reason is the reply: there is no model to answer, and
            // the honest answer to a prompt is the reason there is no
            // answer. The platform's own words, not a sentence invented
            // here.
            self.chat
                .push(ChatRole::Notice, availability.reason().as_bytes());
            self.set_action(availability.reason().as_bytes(), Tone::Unavailable);
            self.rebuild();
            return;
        } else {
            // WHY the band says a request is waiting and not that it is
            // streaming: the service fills the whole answer onto the channel
            // and only then posts the notice that names it, so nothing arrives
            // a token at a time and there is no stream here to describe. What
            // the app genuinely holds at this point is a request it has made
            // and an answer it has not yet been given, which is what the band
            // now says.
            self.set_action(
                b"Generation requested: waiting for the platform's answer.",
                Tone::Accent,
            );
        }

        // WHY the queue is emptied before the prompt is sent: a generation that
        // outlasted the app's wait still answers eventually, and its notice would
        // otherwise be read as the reply to this prompt.
        self.discard_stale_answers();

        let t0 = std::get_ticks().unwrap_or(0);
        let _ = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_GENERATE,
            &prompt[..len],
        );
        let t1 = std::get_ticks().unwrap_or(t0);
        self.ipc_latency_ticks = t1.saturating_sub(t0);

        // WHY the answer is not read here: the service has only just been told, so
        // anything already in the queue is from before this prompt. The event loop
        // reads it once the platform says a message is waiting.
        self.chat.start_stream(&prompt[..len]);
        self.answer_deadline = t1.saturating_add(ANSWER_TIMEOUT_TICKS);
        self.rebuild();
    }

    /// Throws away an answer that arrived after the app stopped waiting for it.
    fn discard_stale_answers(&mut self) {
        let mut rx = [0u8; 64];
        for _ in 0..MAILBOX_DRAIN {
            match std::ipc::receive_any(&mut rx) {
                Err(_) => break,
                Ok((_, std::ai::MSG_TYPE_GENERATE_CHUNK, _)) => continue,
                Ok((_, std::ipc::MSG_INPUT_EVENT, _)) => continue,
                Ok((_, std::ipc::MSG_WINDOW_FOCUSED, _)) => continue,
                Ok(_) => break,
            }
        }
    }

    /// Asks the inference service to cancel the generation in flight.
    pub fn cancel_streaming(&mut self) {
        let sent = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_CANCEL,
            b"CANC",
        )
        .is_ok();

        // WHY the app stops its own wait whatever the service answers: whether
        // the generation is still running is the service's, but sitting and
        // waiting for an answer this person has asked to stop is the app's own
        // state, and closing it needs no permission from anything.
        self.chat.finish_stream();
        self.answer_deadline = 0;

        // WHY the notice names the request and not its outcome: whether
        // anything was cancelled is decided by state only the service holds --
        // it answers an acknowledgement, or says there was no session to
        // cancel, or refuses a sender that does not own the session -- and this
        // app reads none of that back. The naive line here stated the platform
        // had cancelled the generation, which is a verdict the app never
        // observed and would paint even on a turn where there was nothing to
        // cancel or the send itself had been refused.
        if sent {
            self.chat.push(ChatRole::Notice, b"[Cancellation requested]");
            self.set_action(b"Cancellation requested.", Tone::Plain);
        } else {
            // WHY the file's own words for a refused send: the SDK says only
            // that the send failed, so the app reuses the sentence
            // `request_model_install` already uses when the inference service
            // would not take a request rather than inventing an error the
            // platform has no notion of.
            self.chat
                .push(ChatRole::Notice, b"[Cancellation request not sent]");
            self.set_action(
                b"CANCEL: the inference service would not take the request.",
                Tone::Unavailable,
            );
        }
        self.rebuild();
    }

    /// Advances the conversation by whatever the platform has answered.
    ///
    /// WHY the message type is read before the message body: the kernel copies
    /// every input event it dispatches into the focused task's mailbox, and a
    /// touch copy is twenty-four bytes -- which is exactly what an inference
    /// response header looks like. Taking whatever was at the front of the queue
    /// let a tap on the screen be painted as the model's answer.
    ///
    /// WHY there is no invented reply: the answer is the platform's or it is not
    /// there at all. Words this app wrote in a model's voice would be the app's
    /// own, and a transcript that cannot tell the two apart records nothing.
    pub fn step_streaming(&mut self) -> bool {
        if !self.chat.is_streaming {
            return false;
        }
        let mut rx = [0u8; 64];
        match std::ipc::receive_any(&mut rx) {
            Err(_) => self.wait_for_answer(),
            Ok((_, std::ai::MSG_TYPE_GENERATE_CHUNK, len)) => {
                self.take_streamed_answer(&rx[..len])
            }
            Ok((_, std::ai::MSG_TYPE_GENERATE, len)) => self.take_inline_answer(&rx[..len]),
            Ok(_) => false,
        }
    }

    /// Sleeps until the platform has something to say, or the wait runs out.
    fn wait_for_answer(&mut self) -> bool {
        if std::get_ticks().unwrap_or(0) < self.answer_deadline {
            // WHY the touch bit is in the interest mask: the wait is the longest
            // the app can be out of the loop, and a screen that ignored a tap for
            // the length of its own wait would look frozen.
            let _ = std::event_poll(
                std::EVENT_IPC_MSG | std::EVENT_SHM_READABLE | std::EVENT_TOUCH_INPUT,
                ANSWER_POLL_TICKS,
            );
            return false;
        }
        self.answer_deadline = 0;
        self.chat.finish_stream();
        self.chat.push(
            ChatRole::Notice,
            b"The inference service sent no answer. Your prompt is still here.",
        );
        self.set_action(b"No answer received within the wait.", Tone::Unavailable);
        self.rebuild();
        true
    }

    /// Reads the answer the service placed on the channel its notice names.
    fn take_streamed_answer(&mut self, notice_bytes: &[u8]) -> bool {
        let Some(notice) = std::ai::AiStreamNotice::from_bytes(notice_bytes) else {
            return self.report_unreadable_reply();
        };
        let streamed = (notice.streamed_len as usize).min(notice.total_len as usize);
        let mut buf = [0u8; std::ai::ANSWER_SLOT_BYTES];
        let mut collected = 0usize;
        while collected < streamed {
            let read = match std::shm_pipe_receive(notice.pipe_id, &mut buf) {
                Ok(read) if read > 0 => read,
                _ => break,
            };
            let piece = &buf[..read.min(streamed - collected)];
            if collected == 0 {
                self.chat.push(answer_role(notice.status), piece);
            } else {
                self.chat.append_chunk(piece);
            }
            collected += piece.len();
        }
        self.close_answer(notice.status, collected as u32, notice.total_len)
    }

    /// Reads an answer the service sent through the mailbox because no channel was free.
    ///
    /// WHY the header's own length is believed: the mailbox cuts a message at
    /// sixty-four bytes and says nothing, so the only witness left that a reply
    /// was cut is the length the service wrote into its header.
    fn take_inline_answer(&mut self, reply: &[u8]) -> bool {
        let hdr = core::mem::size_of::<std::ai::AiInferenceResponse>();
        if reply.len() < hdr {
            return self.report_unreadable_reply();
        }
        let resp = unsafe {
            core::ptr::read_unaligned(reply.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        let payload = &reply[hdr..];
        if !payload.is_empty() {
            self.chat.push(answer_role(resp.status), payload);
        }
        self.close_answer(resp.status, payload.len() as u32, resp.output_dim)
    }

    /// Closes one answer and says, in bytes, how much of it reached the screen.
    ///
    /// WHY the numbers are shown: the channel, the mailbox and the transcript each
    /// hold their own limit, and a bubble that stops mid-word has to name which one
    /// stopped it -- otherwise the reader wonders whether the model ran out of words.
    fn close_answer(&mut self, status: u32, arrived: u32, offered: u32) -> bool {
        self.answer_deadline = 0;
        self.chat.finish_stream();
        let kept = self.chat.last_text_len() as u32;
        if arrived < offered {
            let mut line = [0u8; 64];
            let mut n = 0;
            put(&mut line, &mut n, b"Cut in transit: ");
            n += format_u32(arrived, &mut line[n..]);
            put(&mut line, &mut n, b" of ");
            n += format_u32(offered, &mut line[n..]);
            put(&mut line, &mut n, b" bytes reached the app.");
            self.set_action(&line[..n], Tone::Unavailable);
        } else if arrived == 0 {
            self.chat
                .push(ChatRole::Notice, b"The service answered with no bytes at all.");
            self.set_action(b"Empty answer from the inference service.", Tone::Unavailable);
        } else if kept < arrived {
            let mut line = [0u8; 64];
            let mut n = 0;
            put(&mut line, &mut n, b"Transcript holds ");
            n += format_u32(kept, &mut line[n..]);
            put(&mut line, &mut n, b" of ");
            n += format_u32(arrived, &mut line[n..]);
            put(&mut line, &mut n, b" bytes.");
            self.set_action(&line[..n], Tone::Unavailable);
        } else if status != std::ai::STATUS_SUCCESS {
            // WHY the reason is not repeated in the strip: the platform's own words
            // are already in the transcript, and the strip only has to say that the
            // request was refused.
            self.set_action(b"The inference service refused the request.", Tone::Unavailable);
        } else {
            self.set_action(b"Answer received whole.", Tone::Good);
        }
        self.rebuild();
        true
    }

    /// Says that a reply arrived and is not one this app can read.
    fn report_unreadable_reply(&mut self) -> bool {
        self.answer_deadline = 0;
        self.chat.finish_stream();
        self.chat
            .push(ChatRole::Notice, b"A reply arrived that this app cannot read.");
        self.set_action(b"Unreadable reply from the inference service.", Tone::Unavailable);
        self.rebuild();
        true
    }

    /// Asks the platform to embed the text the composer holds.
    ///
    /// WHY the composer's text is what goes out: this is the only text a person
    /// types into this app, and a probe that sent a query written here would be
    /// reporting the platform's answer to something nobody asked for. The card
    /// names the bytes that left, which is the shortest text the request can carry
    /// when the composer holds more than one mailbox message can hold.
    ///
    /// WHY the request carries no dimension: `target_dim` of zero is the request
    /// that leaves the service's own default standing, and a number chosen here
    /// would be this app deciding the shape of an answer it has not yet been given.
    ///
    /// WHY the queue is emptied of embedding replies first: on the file-backed
    /// branch the service answers a single request twice -- the vector, then the
    /// sentence that says what the run measured -- and a probe that read the
    /// leftover sentence of the previous request would stamp the words of an older
    /// answer as this one's.
    pub fn run_embedding_probe(&mut self) {
        if self.chat.is_streaming || self.install_deadline != 0 {
            // WHY the plate stands aside: an answer is already being waited for,
            // and a second reader of the same queue would take the message that
            // belongs to the exchange in flight.
            self.set_action(b"An answer is already being waited for.", Tone::Muted);
            return;
        }
        let typed = self.chat.prompt_text();
        let typed_len = typed.len();
        if typed_len == 0 {
            self.set_action(b"Nothing typed to embed.", Tone::Muted);
            return;
        }
        let availability = resolve_inference_service(&self.device.services);
        if !availability.is_available() {
            // WHY the platform's reason is the whole answer: nothing was sent, so
            // there is no reply to report, and the app has no statement to make
            // about an embedding it did not ask for. The words are the same
            // `send_prompt` gives a prompt the device cannot answer.
            self.probe = EmbedProbe::new();
            self.set_action(availability.reason().as_bytes(), Tone::Unavailable);
            self.rebuild();
            return;
        }
        // WHY the text is copied before the send: the composer belongs to the
        // conversation, and the request outlives the borrow of it.
        let mut text = [0u8; PROBE_REQUEST_TEXT_MAX];
        let take = typed_len.min(PROBE_REQUEST_TEXT_MAX);
        text[..take].copy_from_slice(&typed[..take]);
        let mut request = [0u8; MAILBOX_PAYLOAD_MAX];
        let Some(len) = build_embed_request(&text[..take], None, &mut request) else {
            // WHY this cannot happen and is still answered: the buffer is built
            // from the same constant the writer cuts against, so reaching here
            // means the two have come apart -- and an app that fell through would
            // send a request it had not measured.
            self.probe = EmbedProbe::new();
            self.set_action(b"EMBED: this app could not build the request.", Tone::Unavailable);
            self.rebuild();
            return;
        };
        self.discard_stale_embeddings();
        let t0 = std::get_ticks().unwrap_or(0);
        let sent = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_EMBED,
            &request[..len],
        );
        let t1 = std::get_ticks().unwrap_or(t0);
        if sent.is_err() {
            self.probe = EmbedProbe::new();
            self.set_action(
                b"EMBED: the inference service would not take the request.",
                Tone::Unavailable,
            );
            self.rebuild();
            return;
        }
        // WHY the interval is measured around the call and not reported after it:
        // this is the same reading `send_prompt` takes of its own send, and what it
        // costs to hand a message to the kernel is a fact about this device rather
        // than a number the platform publishes.
        self.probe.start_request(
            &text[..len - REQUEST_HEADER_BYTES],
            typed_len,
            None,
            len,
            t0,
            t1.saturating_sub(t0),
            ANSWER_TIMEOUT_TICKS,
        );
        self.set_action(
            b"Embedding requested: waiting for the platform's answer.",
            Tone::Accent,
        );
        self.rebuild();
    }

    /// Throws away an embedding reply that arrived after the app stopped reading.
    ///
    /// WHY the type is checked before the body is kept: the mailbox also holds the
    /// kernel's copy of every input event dispatched to this window, and a touch
    /// copy is twenty-four bytes -- the width of an inference header -- so a drain
    /// that took whatever came next would throw away a real reply or keep a
    /// coordinate.
    fn discard_stale_embeddings(&mut self) {
        let mut rx = [0u8; MAILBOX_PAYLOAD_MAX];
        for _ in 0..MAILBOX_DRAIN {
            match std::ipc::receive_any(&mut rx) {
                Err(_) => break,
                Ok((_, std::ai::MSG_TYPE_EMBED, _)) => continue,
                Ok((_, std::ai::MSG_TYPE_EMBED_CHUNK, _)) => continue,
                Ok((_, std::ipc::MSG_INPUT_EVENT, _)) => continue,
                Ok((_, std::ipc::MSG_WINDOW_FOCUSED, _)) => continue,
                Ok(_) => break,
            }
        }
    }

    /// Advances the embedding probe by whatever the platform has answered.
    ///
    /// WHY only a reply of this app's own message type is taken: the generation
    /// wait and the install wait read the same queue, and the event loop asks this
    /// only while neither of them holds one.
    ///
    /// WHY two types are this app's own: the service answers an embedding with the
    /// vector inline under `MSG_TYPE_EMBED`, and answers a refusal by streaming the
    /// sentence and posting the pointer under `MSG_TYPE_EMBED_CHUNK`. Both are the
    /// reply to this request, and a wait that read only the first would time out on
    /// a refusal the platform had already spent a channel delivering.
    pub fn step_embedding_reply(&mut self) -> bool {
        if self.probe.state != ProbeState::Awaiting {
            return false;
        }
        let mut rx = [0u8; MAILBOX_PAYLOAD_MAX];
        match std::ipc::receive_any(&mut rx) {
            Err(_) => {
                let now = std::get_ticks().unwrap_or(0);
                if now < self.probe.deadline {
                    let _ = std::event_poll(
                        std::EVENT_IPC_MSG | std::EVENT_SHM_READABLE | std::EVENT_TOUCH_INPUT,
                        ANSWER_POLL_TICKS,
                    );
                    return false;
                }
                self.probe.report_no_reply(now);
                self.set_action(b"No embedding reply within the wait.", Tone::Unavailable);
                self.rebuild();
                true
            }
            Ok((_, std::ai::MSG_TYPE_EMBED, len)) => {
                self.take_embedding_reply(&rx[..len]);
                self.rebuild();
                true
            }
            Ok((_, std::ai::MSG_TYPE_EMBED_CHUNK, len)) => {
                // WHY the notice is copied before the channel is read: the sentence
                // lands in the same buffer the mailbox message was received into,
                // and the channel is only named inside it -- the same reason the
                // install reader copies its notice out first.
                let mut notice_bytes = [0u8; std::ai::AiStreamNotice::WIRE_LEN];
                let take = len.min(notice_bytes.len());
                notice_bytes[..take].copy_from_slice(&rx[..take]);
                self.take_streamed_embed_notice(&notice_bytes)
            }
            Ok(_) => false,
        }
    }

    /// Reads one embedding reply into the probe and says what it supports saying.
    fn take_embedding_reply(&mut self, bytes: &[u8]) {
        if self.probe.state != ProbeState::Awaiting {
            // WHY the second reply of a file-backed run is left unread: the
            // service sends the vector and then the sentence that measures the run,
            // and the first is the answer this card reports. Reading the second
            // into the probe would stamp the sentence's byte count as a dimension.
            return;
        }
        let now = std::get_ticks().unwrap_or(0);
        match read_embed_reply(bytes) {
            Some(reply) => {
                self.probe.record(&reply, now);
                if reply.carries_vector() {
                    self.set_action(b"Embedding reply read.", Tone::Good);
                } else {
                    // WHY the strip does not repeat the sentence: the platform's own
                    // words are on the card, and the strip only has to say that the
                    // request was refused -- the same division `close_answer` makes.
                    self.set_action(b"The inference service refused the request.", Tone::Unavailable);
                }
            }
            None => {
                self.probe.report_unreadable(now);
                self.set_action(
                    b"Unreadable reply from the inference service.",
                    Tone::Unavailable,
                );
            }
        }
    }

    /// Reads the refusal the service placed on the channel its notice named.
    ///
    /// WHY the words come off a channel at all: a refusal names its numbers -- the
    /// tensor, the byte counts, the bound -- and runs past the forty bytes a mailbox
    /// message leaves after the reply header, so the service streams it and posts
    /// only the pointer. Until this read the pointer, that sentence was dropped on
    /// the floor and the screen said only that no reply came.
    ///
    /// WHY the drain stops at the notice's own count: the channel hands a slot of
    /// `ANSWER_SLOT_BYTES` at a time and the notice says how many bytes the service
    /// actually filled, so the app takes that many and no more -- and the card,
    /// which keeps the arrived count beside the length the notice declared, says so
    /// when the app's own buffer was the thing that stopped it.
    ///
    /// WHY the words land where a mailbox refusal's land: they are the platform's
    /// sentence either way, and a card that showed the streamed one somewhere else
    /// would make the transport decide what the reader is told.
    fn take_streamed_embed_notice(&mut self, notice_bytes: &[u8]) -> bool {
        let now = std::get_ticks().unwrap_or(0);
        let Some(notice) = std::ai::AiStreamNotice::from_bytes(notice_bytes) else {
            self.probe.report_unreadable(now);
            self.set_action(
                b"Unreadable reply from the inference service.",
                Tone::Unavailable,
            );
            self.rebuild();
            return true;
        };
        let streamed = (notice.streamed_len as usize).min(notice.total_len as usize);
        let mut slot = [0u8; std::ai::ANSWER_SLOT_BYTES];
        let mut words = [0u8; EMBED_NOTICE_TEXT_MAX];
        let mut collected = 0usize;
        while collected < streamed && collected < words.len() {
            let read = match std::shm_pipe_receive(notice.pipe_id, &mut slot) {
                Ok(read) if read > 0 => read,
                _ => break,
            };
            let take = read
                .min(streamed - collected)
                .min(words.len() - collected);
            words[collected..collected + take].copy_from_slice(&slot[..take]);
            collected += take;
        }
        if collected == 0 {
            // WHY this is not an empty sentence on the card: the notice named bytes
            // and the channel handed none, so the app holds no words of the
            // platform's to show, and printing a blank in the place a refusal was
            // promised would read as though the service had refused for no reason.
            self.probe.report_unreadable(now);
            self.set_action(
                b"Unreadable reply from the inference service.",
                Tone::Unavailable,
            );
            self.rebuild();
            return true;
        }
        self.probe
            .record_streamed(notice.status, &words[..collected], notice.total_len, now);
        // WHY the strip does not repeat the sentence: the platform's own words are on
        // the card, and the strip only has to say that the request was refused -- the
        // same division `take_embedding_reply` makes.
        self.set_action(b"The inference service refused the request.", Tone::Unavailable);
        self.rebuild();
        true
    }

    /// Empties the conversation and the prompt.
    pub fn clear_chat(&mut self) {        if self.chat.is_streaming {
            self.cancel_streaming();
        }
        self.chat = ChatSession::new();
        self.set_action(b"Conversation cleared.", Tone::Muted);
        self.rebuild();
    }

    /// Builds the self-test surface.
    fn build_test(&mut self) {
        // WHY the rows are copied out: the builder mutates the body while it
        // walks the rows, and the rows belong to the app.
        let test = self.self_test;
        self.begin_card(b"Self-test", Tone::Accent);
        let passed = test.rows.iter().filter(|row| row.passed).count();
        let mut summary = [0u8; 24];
        let mut n = 0;
        n += format_u32(passed as u32, &mut summary[n..]);
        for byte in b" of " {
            summary[n] = *byte;
            n += 1;
        }
        n += format_u32(agent_manager::SELF_TEST_ROWS as u32, &mut summary[n..]);
        for byte in b" passed" {
            summary[n] = *byte;
            n += 1;
        }
        self.add_chip(
            b"Overall",
            &summary[..n],
            if test.all_passed {
                Tone::Good
            } else {
                Tone::Bad
            },
        );
        for row in test.rows.iter() {
            self.add_chip(
                row.label.as_bytes(),
                if row.passed { b"PASS" } else { b"FAIL" },
                if row.passed { Tone::Good } else { Tone::Bad },
            );
            self.add_wide(row.detail_str().as_bytes(), Tone::Muted);
        }
        self.add_wide(
            b"Every row above holds the value the platform's own function returned.",
            Tone::Muted,
        );

        self.begin_card(b"Agent tools (bounded runtime)", Tone::Plain);
        // WHY the chip reports the platform's answer and not the app's own
        // setting: the app has no setting to report. What it holds is the
        // kernel's answer, and a chip that said "active" while the kernel
        // held nothing would be a claim the screen cannot back up.
        self.add_chip(
            b"Tool authority",
            if self.tool_authority.is_granted() {
                b"[Granted]"
            } else {
                b"[Not held]"
            },
            if self.tool_authority.is_granted() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        for i in 0..self.agent_tools.len() {
            let outcome = self.agent_tools[i];
            self.add_chip(
                outcome.tool.name().as_bytes(),
                if outcome.ok { b"OK" } else { b"UNAVAILABLE" },
                if outcome.ok { Tone::Good } else { Tone::Unavailable },
            );
            self.add_line(outcome.tool.description().as_bytes(), outcome.text().as_bytes(), Tone::Plain);
        }
        self.add_wide(
            b"Tools run bounded execution calling real platform APIs with zero ambient authority.",
            Tone::Muted,
        );
    }

    /// The bottom action bar's buttons for the current surface.
    pub fn actions(&self) -> [ActionButton; 3] {
        let package = self.selected_provider().package_id();
        match self.surface {
            Surface::Models => [
                ActionButton {
                    label: "REFRESH",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "INSTALL",
                    action: Action::Install,
                    enabled: package.is_some(),
                },
                ActionButton {
                    label: "REMOVE",
                    action: Action::Remove,
                    enabled: package.is_some(),
                },
            ],
            Surface::Access => [
                ActionButton {
                    label: "RE-READ GRANTS",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "REFRESH",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Runtime => [
                ActionButton {
                    label: "RE-READ REGISTRY",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Search => [
                ActionButton {
                    label: "RE-READ CONFIG",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "EMBED TEXT",
                    action: Action::ProbeEmbed,
                    // WHY the plate is live before anything is typed: pressing it
                    // is how a reader finds out that the composer is what this
                    // control sends, and the answer is a sentence, not a button
                    // that does nothing -- the same reason SEND stays live with no
                    // model on the device.
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Test => [
                ActionButton {
                    label: "RUN SELF-TEST",
                    action: Action::RunTest,
                    enabled: true,
                },
                ActionButton {
                    label: "RUN TOOLS",
                    action: Action::RunTools,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Chat => [
                if self.chat.is_streaming {
                    ActionButton {
                        label: "STOP",
                        action: Action::CancelStream,
                        enabled: true,
                    }
                } else {
                    ActionButton {
                        label: "SEND",
                        action: Action::Send,
                        // WHY the button is live even when there is no model:
                        // pressing it is how the user finds out, and the
                        // answer is the platform's reason rather than a
                        // button that does nothing.
                        enabled: true,
                    }
                },
                ActionButton {
                    label: "CLEAR",
                    action: Action::ClearChat,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
        }
    }
}

/// Moves the body with the keyboard.
///
/// WHY this exists: a body that can only be scrolled with a finger is a
/// body that cannot be scrolled at all on a device without one, and the
/// keys that move a list everywhere else on the platform moved nothing
/// here. The step is a quarter of the body, so a key press moves the list
/// a visible amount without running off the end of it.
///
/// WHY the Home key is not one of them: the platform answers Home with the
/// home screen before the app is ever consulted, so a list here could not
/// claim the key. Page up is the jump that reaches the app.
fn handle_scroll_key(app: &mut AgentManagerApp, code: u32) -> bool {
    use std::input_key_code;
    let step = (app.body_h() / 4).max(1);
    match code {
        input_key_code::ARROW_DOWN => {
            let next = app.scroll.offset + step;
            app.scroll.offset = next.min(app.scroll.max_offset());
        }
        input_key_code::ARROW_UP => {
            app.scroll.offset = app.scroll.offset.saturating_sub(step);
        }
        input_key_code::PAGE_DOWN => {
            let next = app.scroll.offset + app.body_h();
            app.scroll.offset = next.min(app.scroll.max_offset());
        }
        input_key_code::PAGE_UP => {
            app.scroll.offset = app.scroll.offset.saturating_sub(app.body_h());
        }
        input_key_code::END => {
            app.scroll.offset = app.scroll.max_offset();
        }
        _ => return false,
    }
    true
}

/// What a key press did to the composer.
enum ChatKey {
    /// The composer changed, so the body has to be rebuilt.
    Edited,
    /// The key was not for the composer.
    Ignored,
}

/// Routes a key press to the composer.
///
/// WHY the printable range is ASCII 32..126: that is the whole of the
/// font the platform ships, so a key outside it is a character the
/// screen cannot draw. Accepting one would put a glyph in the prompt
/// that the transcript could never show again.
///
/// WHY the composer refuses a character it cannot hold rather than
/// dropping it: a composer that silently loses the last character typed
/// is a composer that lies about what was written.
fn handle_chat_key(app: &mut AgentManagerApp, code: u32) -> ChatKey {
    use std::input_key_code;
    match code {
        input_key_code::ENTER | 13 => {
            if app.chat.is_streaming {
                app.cancel_streaming();
            } else {
                app.send_prompt();
            }
            ChatKey::Edited
        }
        input_key_code::BACKSPACE | 8 => {
            app.chat.pop_prompt_char();
            app.rebuild();
            ChatKey::Edited
        }
        32..=126 => {
            if app.chat.push_prompt_char(code as u8) {
                app.rebuild();
                ChatKey::Edited
            } else {
                ChatKey::Ignored
            }
        }
        _ => ChatKey::Ignored,
    }
}

/// The model directory, as the screen spells it.
const MODEL_DIRECTORY_LABEL: &[u8] = b"/system/models";

/// The tone for a model directory probe.
fn model_tone(status: ModelProbeStatus) -> Tone {
    match status {
        ModelProbeStatus::Listed => Tone::Good,
        ModelProbeStatus::Absent => Tone::Muted,
        ModelProbeStatus::Refused | ModelProbeStatus::NoAuthority => Tone::Unavailable,
    }
}

/// The tone for a provider's install state.
fn install_tone(state: InstallState) -> Tone {
    match state {
        InstallState::Installed => Tone::Good,
        InstallState::NotInstalled => Tone::Muted,
        InstallState::Unavailable => Tone::Unavailable,
    }
}

/// The tone for a service's state.
fn service_tone(state: ServiceState) -> Tone {
    match state {
        ServiceState::Running => Tone::Good,
        ServiceState::Failed => Tone::Bad,
        ServiceState::Starting => Tone::Unavailable,
        ServiceState::Declared | ServiceState::Stopped => Tone::Muted,
        ServiceState::Unknown => Tone::Unavailable,
    }
}

/// Writes the platform's error code as text.
///
/// WHY the code is named and not just printed: a number on a screen tells
/// the reader nothing, and the difference between "permission denied" and
/// "no such package" is the difference between a capability the app lacks
/// and a package that does not exist.
#[allow(dead_code)]
fn write_error(out: &mut [u8], code: i64) -> usize {
    let mut n = 0;
    let reason: &[u8] = match code {
        -1 => b"permission denied",
        -3 => b"invalid arguments",
        -8 => b"no such package",
        _ => b"the platform refused the request",
    };
    if code < 0 {
        out[n] = b'-';
        n += 1;
    }
    n += format_u32(code.unsigned_abs() as u32, &mut out[n..]);
    for byte in b" (" {
        if n < out.len() {
            out[n] = *byte;
            n += 1;
        }
    }
    for byte in reason {
        if n < out.len() {
            out[n] = *byte;
            n += 1;
        }
    }
    if n < out.len() {
        out[n] = b')';
        n += 1;
    }
    n
}

/// Draws text in the platform's own face.
///
/// WHY the face is chosen by the scale: the guest's bitmap face rasterises a
/// 16x24 cell at integer scales only, so the larger rungs have to be drawn
/// with the platform's outline face or they come out as a doubled bitmap.
/// This is the same choice the platform's own component library makes, and
/// the same reason.
///
/// WHY the background is a parameter: the face composites its glyphs over
/// the colour underneath, and a card whose fill is not the colour the text
/// was given shows a rectangle behind every word.
#[inline(always)]
fn text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    cap_px: u32,
    body: &[u8],
    ink: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if body.is_empty() {
        return None;
    }
    // WHY the outline face at every scale and not only the large ones: the
    // platform scanout is 720x1440, exactly the authoring space this app
    // paints, so composition is 1:1 and the compositor magnifies nothing.
    // The outline face is still preferred because it rasterises the real
    // outline at the requested cap height instead of magnifying the 16x24
    // bitmap fallback face. Most of the text on this screen is a small run,
    // and the outline face is a real outline rather than a scaled bitmap,
    // so it stays sharp at any size.
    //
    // WHY the bitmap face is still here: a run the outline face refuses
    // -- a character the typeface has no outline for -- has to be drawn
    // somehow, and the bitmap face is what it is drawn with.
    let em = std::design::outline_text::em_for_cap_height(cap_px);
    let (ascent, _) = std::design::typeface::line_metrics(em);
    let baseline = y0 + ascent as usize;
    if let Some(run) = std::design::outline_text::OutlineRun::place_styled(x0, baseline, em, body, bold) {
        let (bx, by, bw, bh) = run.bounds();
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return run.pixel(x, y, ink, bg);
        }
        return None;
    }
    // WHY the fallback is the nearest whole scale: the bitmap face draws
    // in whole cells, and a run the outline face refuses -- a character the
    // typeface has no outline for -- has to be drawn at some size. It is
    // the nearest one the face has.
    let fallback = (std::design::outline_text::em_for_cap_height(cap_px) as usize / font::GLYPH_H)
        .max(1);
    font::test_text_aa(x, y, x0, y0, fallback, body, ink, bg, bold)
}

/// The width of `body` at a cap height.
///
/// WHY the face is measured through the typeface and not through the
/// bitmap's own scale: a cap height is not a scale, and the bitmap face
/// can only be asked about whole cells.
fn measure_cap(body: &[u8], cap_px: u32) -> usize {
    let em = std::design::outline_text::em_for_cap_height(cap_px);
    std::design::typeface::text_advance_px(body, em) as usize
}

/// The number of bytes of `body` that fit in `max_px` at the given cap height.
///
/// WHY this is measured and not counted: the platform's face is
/// proportional, so a string's width depends on the letters in it. Counting
/// characters would let a wide string run past the column it belongs to.
fn fit(body: &[u8], max_px: usize, cap_px: u32) -> usize {
    // WHY the byte limit comes first: a run longer than the face's key is
    // refused by the face and drawn by the bitmap, and the bitmap draws at
    // a whole cell, so the run would come out at a different size from
    // every other run on the screen.
    let limit = body.len().min(MAX_RUN_BYTES);
    if measure_cap(&body[..limit], cap_px) <= max_px {
        return limit;
    }
    let mut len = 0usize;
    while len < limit {
        if measure_cap(&body[..len + 1], cap_px) > max_px {
            break;
        }
        len += 1;
    }
    len
}

/// The number of bytes of `body` that fit in `max_px`, copied into `out` and
/// marked when the run was cut.
///
/// WHY the mark belongs here and not in `fit`: `fit` is a measurement and has
/// nowhere to put the mark, so the mark is written while the bytes are being
/// copied, and the two can never disagree about where the run ends.
fn fit_marked(body: &[u8], max_px: usize, cap_px: u32, out: &mut [u8]) -> usize {
    let len = fit(body, max_px, cap_px);
    // WHY the clamp: a destination too small for the run must not be written
    // past, and the returned length is what the caller slices with.
    let len = len.min(out.len());
    out[..len].copy_from_slice(&body[..len]);
    if len < body.len() && len >= 3 {
        out[len - 3..len].copy_from_slice(b"...");
    }
    len
}

// -----------------------------------------------------------------------------
// Painting
// -----------------------------------------------------------------------------

/// The rows the platform's band owns on this screen.
pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

/// Reads the wall clock once and says whether the band's lane changed.
///
/// WHY it returns the change rather than redrawing: the loop repaints only what
/// actually moved, and a clock that answers the same minute costs no frame.
/// WHY an unanswered RTC leaves the lane blank: the read needs the
/// `RealtimeClockRead` capability, and a placeholder time is a measurement the
/// device never took.
fn refresh_clock(app: &mut AgentManagerApp) -> bool {
    let mut clock = [b' '; CLOCK_LEN];
    let mut clock_ok = false;
    if let Some(now) = std::rtc::Reading::now() {
        clock = now.clock_text();
        clock_ok = true;
    }
    let changed = clock_ok != app.clock_ok || (clock_ok && clock != app.clock);
    app.clock = clock;
    app.clock_ok = clock_ok;
    changed
}

/// The colour of one pixel.
fn get_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    // The platform's band, above the app's own bar.
    //
    // WHY the instruments are unmeasured: this app holds no radio, no fuel
    // gauge and no Wi-Fi association -- what it measures is the model
    // directory and the service registry, and those readings are in its cards,
    // not in the band. A filled bar here would be a level no instrument of this
    // app took.
    if y < STATUS_BAR_HEIGHT {
        let band = components::StatusBand::new(W);
        let clock = if app.clock_ok { Some(&app.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
    let base = if y < appbar_bottom() {
        // WHY the bar's own foot is a function of the rung and not the authored
        // `APPBAR_H`: which painter answers a row decides whether the name on the
        // bar is drawn whole, and the bar now grows to the rung's line box. At the
        // authored level the foot is the band the file has always used, so the
        // boundary a Normal-size reader sees has not moved.
        appbar_pixel(x, y, app)
    } else if y < body_top() {
        tab_pixel(x, y, app)
    } else {
        let bottom = app.body_bottom();
        if y < bottom {
            body_pixel(x, y, app).unwrap_or(THEME.surface)
        } else if y < ACTION_TOP {
            composer_pixel(x, y, app)
        } else {
            action_pixel(x, y, app)
        }
    };
    // The overflow sheet is on top of everything else, because a sheet that
    // is behind the thing it covers is a panel, not a sheet.
    if let Some(c) = sheet_pixel(x, y, app) {
        return c;
    }
    base
}

/// The app bar: the way out, the surface's name, and the two controls.
///
/// WHY the band is the page's colour and not a card's: the design the
/// showcases describe puts the app bar on the background and lets the
/// cards float on it. A white bar across the top of a grey page is a
/// different design.
fn appbar_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;

    // The back control.
    //
    // WHY the app has one: it is launched from the launcher's drawer, and
    // without a back control the only way out of it is the system gesture.
    // It is drawn in the platform's own place and tested with the
    // platform's own hit region, so the button a person sees and the
    // button a person presses are the same button.
    //
    // WHY the colours come from the theme rather than from this call: the chip
    // used to be filled with `THEME.surface` and ringed with `THEME.border`,
    // which is the page wearing the page -- nothing of it survived the screen.
    if let Some(c) = std::design::back_button::pixel_for_theme(x, y, &THEME) {
        colour = c;
    }

    // The surface's name. It is the largest thing on the bar, because the
    // name of the screen is the first thing a person looks for.
    //
    // WHY it is measured before it is painted: every rung on the ladder can
    // move a name's width and the bar's own span is fixed by the two controls
    // at its right, and `draw_label` on this platform clips nothing. An
    // unmeasured run that outgrew the span would not wrap -- its last letters
    // would land on the search button's circle and read as part of it.
    let title = app.surface.title();
    let mut shown = [0u8; APPBAR_TITLE_BUF];
    let len = fit_marked(title.as_bytes(), APPBAR_TITLE_MAX_W, cap_title(), &mut shown);
    if let Some(c) = text(
        x,
        y,
        APPBAR_TITLE_X,
        appbar_title_y(),
        cap_title(),
        &shown[..len],
        THEME.on_surface,
        THEME.surface,
        true,
    ) {
        colour = c;
    }

    // The search control, which leads to the search surface.
    if let Some(c) = icon_button_px(x, y, appbar_search_rect(), Icon::Search) {
        colour = c;
    }

    // The overflow control, which opens the sheet.
    if let Some(c) = icon_button_px(x, y, appbar_more_rect(), Icon::More) {
        colour = c;
    }

    colour
}

/// The colour of `(x, y)` if it is an app bar icon button.
///
/// WHY the button is a circle on a hairline and not a bare glyph: a glyph
/// on the page is a label, and a label that is also a control is a control
/// nobody presses.
fn icon_button_px(x: usize, y: usize, rect: Rect, icon: Icon) -> Option<u32> {
    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let mut colour = monochrome::circle_px(
        x,
        y,
        cx,
        cy,
        rect.w / 2,
        THEME.surface_container,
        Some(THEME.border),
    )?;
    let glyph = Rect::new(cx - 11, cy - 11, 22, 22);
    if let Some(c) = monochrome::icon_px(x, y, glyph, icon, THEME.on_surface) {
        colour = c;
    }
    Some(colour)
}

/// The app bar's search control.
fn appbar_search_rect() -> Rect {
    Rect::new(W - GUTTER - ICON_BTN * 2 - ICON_BTN_GAP, ICON_BTN_TOP, ICON_BTN, ICON_BTN)
}

/// The app bar's overflow control.
fn appbar_more_rect() -> Rect {
    Rect::new(W - GUTTER - ICON_BTN, ICON_BTN_TOP, ICON_BTN, ICON_BTN)
}

// ---------------------------------------------------------------------------
// The overflow sheet
// ---------------------------------------------------------------------------

/// The sheet's width.
const SHEET_W: usize = 272;
/// The sheet's row height.
/// A sheet row's height. It follows the type for the same reason.
///
/// WHY it is a function of the rung: the row holds a body-rung line centred in
/// it, and the sheet is the one plate on this screen that is grown by the same
/// arithmetic both above and below -- its height, its rows and the hit test that
/// answers them all read the published level, so no row can be shorter than the
/// word it carries.
const fn sheet_row_h_at(level: u8) -> usize {
    line_at(SCALE_BODY, level) + 24
}
#[inline]
fn sheet_row_h() -> usize {
    sheet_row_h_at(level())
}
/// The sheet's padding.
const SHEET_PAD: usize = 8;

/// The rows the sheet holds.
///
/// WHY the sheet holds the actions and not the surfaces: the surfaces are
/// already in the segmented control, and a sheet that repeats them is a
/// sheet with nothing in it.
const SHEET_ROWS: [(Icon, &str, Action); 4] = [
    (Icon::Refresh, "Re-measure device", Action::Refresh),
    (Icon::Activity, "Run self-test", Action::RunTest),
    (Icon::Send, "Open chat", Action::OpenChat),
    (Icon::Close, "Clear conversation", Action::ClearChat),
];

/// The sheet's rectangle.
///
/// WHY it hangs from the tab row's top rather than the authored bar's: the sheet
/// is opened from the bar and covers the control beneath it, and the control
/// moves down when the bar's name needs the rows. At the authored level the two
/// are the same row -- `TAB_Y == APPBAR_H + 8` is the gate above -- so the sheet
/// is where it always was, and its height grows with the row the labels are cut
/// at.
fn sheet_rect() -> Rect {
    let height = SHEET_PAD * 2 + SHEET_ROWS.len() * sheet_row_h();
    Rect::new(W - GUTTER - SHEET_W, tab_y(), SHEET_W, height)
}

/// The colour of `(x, y)` if it is the overflow sheet.
fn sheet_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    if !app.sheet_open {
        return None;
    }
    let rect = sheet_rect();
    if !rect.contains(x, y) {
        return None;
    }
    let mut colour =
        monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border))?;
    for (index, (icon, label, _)) in SHEET_ROWS.iter().enumerate() {
        let row_y = rect.y + SHEET_PAD + index * sheet_row_h();
        let row = Rect::new(rect.x, row_y, rect.w, sheet_row_h());
        if !row.contains(x, y) {
            continue;
        }
        let glyph = Rect::new(rect.x + 16, row_y + (sheet_row_h() - 22) / 2, 22, 22);
        if let Some(c) = monochrome::icon_px(x, y, glyph, *icon, THEME.on_surface) {
            colour = c;
        }
        if let Some(c) = text(
            x,
            y,
            rect.x + 50,
            row_y + (sheet_row_h() - line_body()) / 2,
            cap_body(),
            label.as_bytes(),
            THEME.on_surface,
            THEME.surface_container,
            false,
        ) {
            colour = c;
        }
    }
    Some(colour)
}

// ---------------------------------------------------------------------------
// The floating action button
// ---------------------------------------------------------------------------

/// The floating action button's rectangle.
///
/// WHY it floats over the body instead of sitting in the action bar: a
/// button that is in the bar is a button in a row, and a row of buttons
/// has no primary one. The floating button is the screen's one obvious
/// action, and it is the one a thumb reaches for first.
fn fab_rect(app: &AgentManagerApp) -> Rect {
    // WHY the chat surface has no floating button: the composer carries a
    // send button already, and two controls that do the same thing on one
    // screen is one control too many.
    if app.surface == Surface::Chat {
        return Rect::new(0, 0, 0, 0);
    }
    let bottom = app.body_bottom();
    Rect::new(
        W - GUTTER - FAB_R * 2 - FAB_MARGIN,
        bottom - FAB_R * 2 - FAB_MARGIN,
        FAB_R * 2,
        FAB_R * 2,
    )
}

/// The action the floating button performs on a surface.
fn fab_action(surface: Surface) -> Action {
    match surface {
        Surface::Models => Action::Install,
        Surface::Chat => Action::Send,
        _ => Action::Refresh,
    }
}

/// The glyph the floating button carries on a surface.
fn fab_icon(surface: Surface) -> Icon {
    match surface {
        Surface::Models => Icon::Plus,
        Surface::Chat => Icon::Send,
        _ => Icon::Refresh,
    }
}

/// The colour of `(x, y)` if it is the floating action button.
fn fab_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    let rect = fab_rect(app);
    if rect.w == 0 || rect.h == 0 {
        return None;
    }
    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let mut colour = monochrome::circle_px(x, y, cx, cy, FAB_R, THEME.primary, None)?;
    let glyph = Rect::new(cx - 13, cy - 13, 26, 26);
    if let Some(c) = monochrome::icon_px(x, y, glyph, fab_icon(app.surface), THEME.on_primary) {
        colour = c;
    }
    Some(colour)
}

/// The segmented control.
fn tab_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;
    // WHY the gap count is derived rather than written down: the tab row
    // has a gap between every pair of segments, so a row of N segments
    // has N-1 gaps. The number was written down, and the last tab in the
    // row ran past the content column the moment a sixth surface was
    // added.
    let segment_w = (CONTENT_W - (Surface::COUNT - 1) * TAB_GAP) / Surface::COUNT;
    let index = (x.saturating_sub(GUTTER)) / (segment_w + TAB_GAP);
    if index < Surface::COUNT {
        let seg_x = GUTTER + index * (segment_w + TAB_GAP);
        let seg = Rect::new(seg_x, tab_y(), segment_w, TAB_H);
        if seg.contains(x, y) {
            let selected = app.surface.index() == index;
            let (fill, ink) = if selected {
                (THEME.primary, THEME.on_primary)
            } else {
                (THEME.surface_container, THEME.on_surface_variant)
            };
            colour = monochrome::chip_px(
                x,
                y,
                seg,
                fill,
                if selected { None } else { Some(THEME.border) },
            )
            .unwrap_or(fill);

            let label = Surface::ALL[index].tab_label().as_bytes();
            let text_w = measure_cap(label, cap_caption());
            let text_x = seg_x + segment_w.saturating_sub(text_w) / 2;
            if let Some(c) = text(
                x,
                y,
                text_x,
                tab_y() + (TAB_H - line_caption()) / 2,
                cap_caption(),
                label,
                ink,
                fill,
                selected,
            ) {
                colour = c;
            }
        }
    }
    colour
}

/// The scrollable body.
fn body_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    // The floating action button is on top of the body, on top of the last
    // card, because that is where a thumb reaches for it.
    if let Some(c) = fab_pixel(x, y, app) {
        return Some(c);
    }
    body_cards_pixel(x, y, app)
}

/// The scrollable body's cards.
fn body_cards_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    let offset = app.scroll.offset;
    let content_y = y - body_top() + offset;

    for index in 0..app.card_count as usize {
        let card = app.cards[index];
        let card_y = card.y as usize;
        let card_h = card.height as usize;
        if content_y < card_y || content_y >= card_y + card_h {
            continue;
        }

        let top = body_top() as i64 + card_y as i64 - offset as i64;
        let bottom = top + card_h as i64;
        if bottom <= 0 || top >= H as i64 {
            return None;
        }
        let rect_y = top.max(0) as usize;
        let rect_h = (bottom.min(H as i64) - rect_y as i64) as usize;

        if card.kind != CARD_PANEL {
            return bubble_pixel(x, y, app, card, rect_y, rect_h);
        }

        let rect = Rect::new(GUTTER, rect_y, CONTENT_W, rect_h);
        // WHY the card is the platform's monochrome card: a card that invents
        // its own fill and its own hairline is a card a third-party app
        // cannot match, and the design system already says what a card
        // looks like.
        let mut colour =
            monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border))?;

        // The heading.
        let title_top = card_y + CARD_PAD_TOP;
        if content_y < card_y + lines_top() {
            let title = &card.title[..card.title_len as usize];
            let mut shown = [0u8; 256];
            let len = fit_marked(title, CONTENT_W - 2 * CARD_PAD_X, cap_section(), &mut shown);
            if let Some(c) = text(
                x,
                y,
                GUTTER + CARD_PAD_X,
                body_top() + title_top - offset,
                cap_section(),
                &shown[..len],
                card.title_tone.colour(),
                THEME.surface_container,
                // WHY the heading is bold: the heading is the name of the
                // thing the rows under it belong to, and a name has to be
                // findable before the rows are read.
                true,
            ) {
                colour = c;
            }
            return Some(colour);
        }

        // The lines.
        let lines_top = card_y + lines_top();
        let mut cursor = lines_top;
        for line_index in 0..card.line_count as usize {
            let line = app.lines[card.line_start as usize + line_index];
            let height = line.rows() * row_h();
            if content_y >= cursor && content_y < cursor + height {
                let screen_y = body_top() + cursor - offset;
                if line.wide {
                    let row = (content_y - cursor) / row_h();
                    let (start, end) = line.row_range(row);
                    if start < end {
                        let body = &line.value[start..end];
                        let mut shown = [0u8; 256];
                        let len = fit_marked(body, WIDE_W, cap_caption(), &mut shown);
                        // WHY the row's own y and not the line's: a wide
                        // line that wraps has more than one physical row, and
                        // every one of them is a row of its own.
                        if let Some(c) = text(
                            x,
                            y,
                            GUTTER + CARD_PAD_X,
                            screen_y + row * row_h(),
                            cap_caption(),
                            &shown[..len],
                            line.tone.colour(),
                            THEME.surface_container,
                            false,
                        ) {
                            colour = c;
                        }
                    }
                } else {
                    let label = &line.label[..line.label_len as usize];
                    let mut shown = [0u8; 256];
                    let len = fit_marked(label, LABEL_W - LABEL_GAP, cap_caption(), &mut shown);
                    if let Some(c) = text(
                        x,
                        y,
                        GUTTER + CARD_PAD_X,
                        screen_y + (row_h() - line_caption()) / 2,
                        cap_caption(),
                        &shown[..len],
                        THEME.on_surface_variant,
                        THEME.surface_container,
                        false,
                    ) {
                        colour = c;
                    }
                    let value = &line.value[..line.value_len as usize];
                    if line.chip {
                        // WHY a chip and not coloured text: a state written
                        // in colour is a state a person has to read, and a
                        // state written as a dot and a word is a state a
                        // person can find at a glance.
                        if let Some(c) = chip_px(
                            x,
                            y,
                            GUTTER + CONTENT_W - CARD_PAD_X,
                            screen_y + (row_h() - chip_h()) / 2,
                            value,
                            line.tone,
                        ) {
                            colour = c;
                        }
                    } else {
                        let mut shown = [0u8; 256];
                        // WHY the mark is not written here: a value cut in
                        // the middle of a word reads as a word, and the
                        // reader is left with a different name for the thing
                        // than the platform gave, so the cut and the mark are
                        // decided in one place.
                        let len = fit_marked(value, VALUE_W, cap_body(), &mut shown);
                        // WHY the value is right-aligned: a value that starts
                        // at a fixed column leaves a ragged edge, and a list of
                        // ragged values reads as a table of unrelated things.
                        // Aligned to the row's edge, the values form a column
                        // a person can scan down.
                        let value_w = measure_cap(&shown[..len], cap_body());
                        let value_x = (GUTTER + CONTENT_W - CARD_PAD_X).saturating_sub(value_w).max(VALUE_X);
                        if let Some(c) = text(
                            x,
                            y,
                            value_x,
                            screen_y + (row_h() - line_body()) / 2,
                            cap_body(),
                            &shown[..len],
                            line.tone.colour(),
                            THEME.surface_container,
                            false,
                        ) {
                            colour = c;
                        }
                    }
                }
                break;
            }
            cursor += height;
        }
        return Some(colour);
    }

    Some(THEME.surface)
}

/// The colour of `(x, y)` if it is a message bubble.
///
/// WHY the person is on the right and the platform is on the left: the
/// design the showcases describe says so, and a conversation drawn the
/// other way round reads as a log rather than as a conversation.
fn bubble_pixel(
    x: usize,
    y: usize,
    app: &AgentManagerApp,
    card: Card,
    rect_y: usize,
    rect_h: usize,
) -> Option<u32> {
    let width = (card.width as usize).min(CONTENT_W);
    let left = if card.kind == CARD_BUBBLE_ME {
        W - GUTTER - width
    } else {
        GUTTER
    };
    let rect = Rect::new(left, rect_y, width, rect_h);
    let (fill, ink) = if card.kind == CARD_BUBBLE_ME {
        (THEME.primary, THEME.on_primary)
    } else {
        (THEME.surface_container_low, THEME.on_surface)
    };
    let mut colour = monochrome::card_px(x, y, rect, fill, None)?;

    let mut cursor = rect_y + BUBBLE_PAD;
    for line_index in 0..card.line_count as usize {
        let line = app.lines[card.line_start as usize + line_index];
        for row in 0..line.rows() {
            let (start, end) = line.row_range(row);
            let body = &line.value[start..end];
            let mut shown = [0u8; 256];
            let len = fit_marked(
                body,
                width.saturating_sub(2 * BUBBLE_PAD),
                cap_caption(),
                &mut shown,
            );
            if let Some(c) = text(
                x,
                y,
                left + BUBBLE_PAD,
                cursor,
                cap_caption(),
                &shown[..len],
                ink,
                fill,
                false,
            ) {
                colour = c;
            }
            cursor += row_h();
        }
    }
    Some(colour)
}

/// The gap a chip's fill keeps above and below the caption it holds.
const CHIP_PAD_Y: usize = 2;
/// The height of a status chip.
///
/// WHY it is the caption's own line box plus its padding and not a number
/// written down: a chip holds one caption run, and the run reserves the caption
/// rung's whole line box whatever the chip was cut for, so a height written as
/// a literal is a promise the chip can break the moment the ladder moves --
/// and the screen paints no clip, so the ink lands on the card.
///
/// WHY one name for both places that need it: the chip's fill and the row
/// that centres the chip are the same height, and two numbers for it are two
/// numbers that can disagree without anything saying so.
///
/// WHY it is a function of the rung: the word inside it is drawn at the
/// published level, so the fill has to be cut at the same level or the mark
/// stands taller than the pill it is set in.
const fn chip_h_at(level: u8) -> usize {
    line_at(SCALE_CAPTION, level) + 2 * CHIP_PAD_Y
}
#[inline]
fn chip_h() -> usize {
    chip_h_at(level())
}
/// What a chip spends around the word it holds: the leading inset, the dot,
/// the gap after the dot and the trailing inset.
///
/// WHY it is one number: the chip's width is this chrome plus whatever the
/// measured word needs, and the word's bound is the same chrome subtracted
/// from the column. Written twice as two halves they can disagree, and the
/// disagreement is a word cut shorter than the fill that was drawn for it.
const CHIP_CHROME: usize = 10 + 8 + 6 + 10;

// WHY at compile time: the chip is centred inside a row and painted with no
// clip, so a chip taller than the row that carries it is a chip whose fill
// lands on the row above and the row below. `chip_h_at` is built from the caption
// rung and `row_h_at` from the body rung, so the two can part company without
// anything on screen saying so until the ladder moves -- and now that both
// follow the person's text size, the check has to be made at every selectable
// level rather than once against a `const`.
const fn chip_fits_its_row_at_every_level() -> bool {
    let mut level = 0;
    while level <= 3 {
        if chip_h_at(level) > row_h_at(level) {
            return false;
        }
        level += 1;
    }
    true
}
const _: () = assert!(chip_fits_its_row_at_every_level());

/// The colour of `(x, y)` if it is a status chip whose right edge is at
/// `right_x`.
fn chip_px(
    x: usize,
    y: usize,
    right_x: usize,
    top: usize,
    label: &[u8],
    tone: Tone,
) -> Option<u32> {
    // WHY the word is trimmed before the fill is cut: the chip is right-anchored
    // and grows to the left, so a state word wider than the value column pushed
    // its own fill into the label column and painted the label through the dot.
    // Trimming it is the only case this control can have too much to say.
    let mut shown = [0u8; MAX_RUN_BYTES];
    let len = fit_marked(label, VALUE_W.saturating_sub(CHIP_CHROME), cap_caption(), &mut shown);
    let label = &shown[..len];
    let text_w = measure_cap(label, cap_caption());
    let width = CHIP_CHROME + text_w;
    let left = right_x.saturating_sub(width);
    let rect = Rect::new(left, top, width, chip_h());
    let mut colour = monochrome::chip_px(x, y, rect, THEME.surface_container_low, None)?;
    let dot_cx = left + 10 + 4;
    let dot_cy = top + chip_h() / 2;
    if let Some(c) = monochrome::circle_px(x, y, dot_cx, dot_cy, 4, tone.colour(), None) {
        colour = c;
    }
    if let Some(c) = text(
        x,
        y,
        dot_cx + 10,
        // WHY the caption is centred against the chip: the caption reserves
        // its line box from the row it is stamped on, and a stamp that
        // leaves the box taller than the chip spills the word over the fill.
        top + (chip_h() - line_caption()) / 2,
        cap_caption(),
        label,
        tone.colour(),
        THEME.surface_container_low,
        false,
    ) {
        colour = c;
    }
    Some(colour)
}

/// The composer band, on the chat surface.
///
/// WHY the composer is pinned rather than scrolled: a composer that scrolls
/// away is a composer that cannot be reached while reading the answer, and
/// the answer is the reason the person is here.
fn composer_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;
    if app.surface != Surface::Chat {
        return colour;
    }
    let top = ACTION_TOP - composer_h();
    if y == top {
        colour = THEME.border;
    }
    let field_y = top + composer_h().saturating_sub(composer_field_h()) / 2;
    let send_cx = W - GUTTER - COMPOSER_SEND_R;
    let send_cy = top + composer_h() / 2;
    let field_right = send_cx.saturating_sub(COMPOSER_SEND_R + 12);
    let field = Rect::new(
        GUTTER,
        field_y,
        field_right.saturating_sub(GUTTER),
        composer_field_h(),
    );
    if field.contains(x, y) {
        if let Some(c) =
            monochrome::control_px(x, y, field, THEME.surface_container, Some(THEME.border))
        {
            colour = c;
        }
        let prompt = app.chat.prompt_text();
        // WHY the prompt's ink is the secondary ink and not the dimmed one:
        // on this light page `muted` measures 2.4:1 against the field's white,
        // which is the contrast of a thing you cannot read, and this is the only
        // sentence saying what the field is for. `muted` stays what it is for --
        // chrome the person cannot press.
        let (body, ink) = if app.chat.prompt_is_empty() {
            (&b"Type a message..."[..], THEME.on_surface_variant)
        } else {
            (prompt, THEME.on_surface)
        };
        let mut shown = [0u8; 256];
        let len = fit_marked(body, field.w.saturating_sub(28), cap_body(), &mut shown);
        if let Some(c) = text(
            x,
            y,
            field.x + 14,
            field_y + (composer_field_h() - line_body()) / 2,
            cap_body(),
            &shown[..len],
            ink,
            THEME.surface_container,
            false,
        ) {
            colour = c;
        }
    }
    if let Some(c) = monochrome::circle_px(
        x,
        y,
        send_cx,
        send_cy,
        COMPOSER_SEND_R,
        THEME.primary,
        None,
    ) {
        colour = c;
    }
    if let Some(c) = monochrome::icon_px(
        x,
        y,
        Rect::new(send_cx - 12, send_cy - 12, 24, 24),
        Icon::Send,
        THEME.on_primary,
    ) {
        colour = c;
    }
    colour
}

/// The bottom action bar.
fn action_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.overlay;
    if y == ACTION_TOP {
        colour = THEME.overlay_border;
    }

    let buttons = app.actions();
    let live = buttons.iter().filter(|button| !button.label.is_empty()).count();
    let row = action_row(live);
    let mut index = 0usize;
    for button in buttons.iter() {
        if button.label.is_empty() {
            continue;
        }
        // WHY the row answers for the plate: the rect painted here is the rect the
        // tap reads, from one description of the bar. The arithmetic used to be
        // written twice -- once here and once in the hit test -- and the two copies
        // are how a plate and its press part company.
        let rect = row.slot(index);
        if rect.contains(x, y) {
            let (fill, ink) = if !button.enabled {
                (THEME.surface_container, THEME.muted)
            } else if index == 0 {
                (THEME.primary, THEME.on_primary)
            } else {
                (THEME.surface_container_high, THEME.on_surface)
            };
            // WHY the primary button has no hairline: a light line
            // around a black button is a seam, and a seam on the one
            // control a person is meant to press is a thing that looks
            // like a mistake.
            colour = monochrome::chip_px(
                x,
                y,
                rect,
                fill,
                if index == 0 { None } else { Some(THEME.border) },
            )
            .unwrap_or(fill);
            let text_w = measure_cap(button.label.as_bytes(), cap_caption());
            let text_x = rect.x + rect.w.saturating_sub(text_w) / 2;
            if let Some(c) = text(
                x,
                y,
                text_x,
                BUTTON_Y + (BUTTON_H - line_caption()) / 2,
                cap_caption(),
                button.label.as_bytes(),
                ink,
                fill,
                true,
            ) {
                colour = c;
            }
        }
        index += 1;
    }

    // The last action's result.
    let message = &app.action_log[..app.action_log_len];
    if y >= result_y() && y < result_y() + line_caption() + 4 {
        let len = fit(message, CONTENT_W, cap_caption());
        // WHY the mark: a status line that stops in the middle of a word
        // reads as a word, and a reader who takes it for one is reading a
        // sentence the platform never said. The mark says the sentence
        // continues.
        let mut shown = [0u8; ACTION_LOG_MAX];
        shown[..len].copy_from_slice(&message[..len]);
        let len = if len < message.len() && len >= 3 {
            shown[len - 3..len].copy_from_slice(b"...");
            len
        } else {
            len
        };
        if let Some(c) = text(
            x,
            y,
            GUTTER,
            result_y(),
            cap_caption(),
            &shown[..len],
            app.action_tone.colour(),
            THEME.overlay,
            false,
        ) {
            colour = c;
        }
    }

    // The home indicator: the shell's own mark, in the shell's own rows.
    if let Some(ink) = std::design::home_band::pixel(x, y, THEME.overlay_border) {
        return ink;
    }

    colour
}

// -----------------------------------------------------------------------------
// Input
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    All,
}

/// What a tap did.
enum Tap {
    None,
    Redraw,
    Leave(&'static str),
}

/// Which body line a tap landed on, and what it would do.
fn hit_body(app: &AgentManagerApp, x: usize, y: usize) -> Option<i8> {
    if x < GUTTER || x >= GUTTER + CONTENT_W {
        return None;
    }
    let content_y = y - body_top() + app.scroll.offset;
    for index in 0..app.card_count as usize {
        let card = app.cards[index];
        let card_y = card.y as usize;
        let card_h = card.height as usize;
        if content_y < card_y || content_y >= card_y + card_h {
            continue;
        }
        // WHY a bubble is skipped: a bubble is a message, and a message is
        // not something a tap selects.
        if card.kind != CARD_PANEL {
            continue;
        }
        let mut cursor = card_y + lines_top();
        for line_index in 0..card.line_count as usize {
            let line = app.lines[card.line_start as usize + line_index];
            let height = line.rows() * row_h();
            if content_y >= cursor && content_y < cursor + height {
                if line.action >= 0 {
                    return Some(line.action);
                }
                return None;
            }
            cursor += height;
        }
        return None;
    }
    None
}

/// Handles a tap.
fn handle_tap(app: &mut AgentManagerApp, x: u32, y: u32) -> Tap {
    let ux = x as usize;
    let uy = y as usize;

    // The back control in the app bar.
    //
    // WHY the rows are the control's own: the region used to reach up into the
    // band and stop short of the control's bottom, so a tap on the lower half
    // of the button the reader was looking at did nothing and a tap on the band
    // did.
    //
    // WHY the columns are the control's own, too: the bar paints the chip
    // through `back_button::pixel_for_theme`, whose plate is 24..72, while this
    // press ran to 88. The 16 columns past the plate are the gap the bar keeps
    // before the surface's name at `APPBAR_TITLE_X`, so the old band made an
    // invisible strip answer as a button. One rect now serves the painter and
    // this test.
    if std::design::back_button::contains(ux, uy) {
        return Tap::Leave("[AGENT_MGR] Back button tapped. Returning to the launcher.");
    }

    // The home indicator: the box the mark stands in, answered ahead of every
    // band of this app's own that reaches below the result line.
    if HOME_HIT.contains(ux, uy) {
        return Tap::Leave("[AGENT_MGR] Home indicator tapped. Returning to the launcher.");
    }

    // The overflow sheet, which is on top of everything.
    //
    // WHY a tap outside the sheet closes it and does nothing else: a sheet
    // that swallows every tap under it is a sheet the reader cannot get
    // past, and a sheet that acted on the tap under it would be a button
    // the person cannot see they are pressing.
    if app.sheet_open {
        let sheet = sheet_rect();
        if !sheet.contains(ux, uy) {
            app.sheet_open = false;
            return Tap::Redraw;
        }
        for (index, (_icon, _label, action)) in SHEET_ROWS.iter().enumerate() {
            let row_y = sheet.y + SHEET_PAD + index * sheet_row_h();
            if uy >= row_y && uy < row_y + sheet_row_h() {
                app.sheet_open = false;
                match action {
                    Action::Refresh => app.refresh(),
                    Action::RunTest => app.run_self_test(),
                    Action::OpenChat => {
                        app.surface = Surface::Chat;
                        app.scroll = ScrollState::new(app.content_height as usize, band_height(), 0);
                        app.rebuild();
                    }
                    Action::ClearChat => app.clear_chat(),
                    _ => {}
                }
                return Tap::Redraw;
            }
        }
        return Tap::None;
    }

    // The app bar's search control.
    if appbar_search_rect().contains(ux, uy) {
        if app.surface != Surface::Search {
            app.surface = Surface::Search;
            app.scroll = ScrollState::new(app.content_height as usize, band_height(), 0);
            app.rebuild();
        }
        return Tap::Redraw;
    }

    // The app bar's overflow control.
    if appbar_more_rect().contains(ux, uy) {
        app.sheet_open = true;
        return Tap::Redraw;
    }

    // The segmented control.
    if uy >= tab_y() && uy < tab_y() + TAB_H {
        // WHY the gap count is derived: see the note at the tab row's
        // own geometry.
        let segment_w = (CONTENT_W - (Surface::COUNT - 1) * TAB_GAP) / Surface::COUNT;
        let index = (ux.saturating_sub(GUTTER)) / (segment_w + TAB_GAP);
        if let Some(surface) = Surface::from_index(index) {
            if surface != app.surface {
                app.surface = surface;
                app.scroll = ScrollState::new(app.content_height as usize, band_height(), 0);
                app.rebuild();
                return Tap::Redraw;
            }
        }
        return Tap::None;
    }

    // The bottom action bar.
    //
    // WHY the row is asked rather than the arithmetic redone: the plates were
    // measured here from `BUTTON_Y` and a width computed a second time from the
    // live count, and a band that answers a row the painter never drew is the
    // defect the tab row already carried. One description, two readers.
    let buttons = app.actions();
    let live = buttons.iter().filter(|button| !button.label.is_empty()).count();
    if let Some(index) = action_row(live).tapped(ux, uy) {
        let Some(button) = buttons
            .iter()
            .filter(|button| !button.label.is_empty())
            .nth(index)
        else {
            return Tap::None;
        };
        if !button.enabled {
            return Tap::None;
        }
        match button.action {
            Action::Refresh => app.refresh(),
            Action::Install => app.install_selected(),
            Action::Remove => app.remove_selected(),
            Action::RunTest => app.run_self_test(),
            Action::RunTools => app.run_tools(),
            Action::CancelStream => app.cancel_streaming(),
            Action::Send => app.send_prompt(),
            Action::ProbeEmbed => app.run_embedding_probe(),
            Action::ClearChat => app.clear_chat(),
            Action::OpenChat => {
                app.surface = Surface::Chat;
                app.scroll = ScrollState::new(app.content_height as usize, band_height(), 0);
                app.rebuild();
            }
        }
        return Tap::Redraw;
    }
    if uy >= BUTTON_Y && uy < BUTTON_Y + BUTTON_H {
        return Tap::None;
    }

    // The composer's send button.
    if app.surface == Surface::Chat && uy >= ACTION_TOP - composer_h() && uy < ACTION_TOP {
        let send_cx = W - GUTTER - COMPOSER_SEND_R;
        let send_cy = ACTION_TOP - composer_h() + composer_h() / 2;
        let dx = ux as i64 - send_cx as i64;
        let dy = uy as i64 - send_cy as i64;
        if dx * dx + dy * dy <= (COMPOSER_SEND_R * COMPOSER_SEND_R) as i64 {
            if app.chat.is_streaming {
                app.cancel_streaming();
            } else {
                app.send_prompt();
            }
            return Tap::Redraw;
        }
        return Tap::None;
    }

    // The floating action button.
    if app.surface != Surface::Chat && uy >= body_top() && uy < app.body_bottom() {
        let fab = fab_rect(app);
        let cx = fab.x + fab.w / 2;
        let cy = fab.y + fab.h / 2;
        let dx = ux as i64 - cx as i64;
        let dy = uy as i64 - cy as i64;
        if dx * dx + dy * dy <= (FAB_R * FAB_R) as i64 {
            match fab_action(app.surface) {
                Action::Install => app.install_selected(),
                Action::Send => app.send_prompt(),
                _ => app.refresh(),
            }
            return Tap::Redraw;
        }
    }

    // The body.
    if uy >= body_top() && uy < app.body_bottom() {
        if let Some(action) = hit_body(app, ux, uy) {
            app.select_provider(action as usize);
            return Tap::Redraw;
        }
        return Tap::None;
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// The shell
// -----------------------------------------------------------------------------

/// Paints the screen.
fn repaint(surface: u32, app: &AgentManagerApp) {
    std::design::outline_text::reset_slot_pool();
    let mut chunk = [0u32; CHUNK];
    let mut offset = 0usize;
    while offset < W * H {
        let count = (W * H - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for index in 0..count {
            chunk[index] = get_pixel(x, y, app);
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

/// Paints the platform band's rows and nothing else.
///
/// WHY a lane of its own: the clock moves while nobody is touching the screen,
/// and answering a minute with a whole-screen repaint costs the reader a frame
/// for five glyphs' worth of change.
fn repaint_band(surface: u32, app: &AgentManagerApp) {
    let mut row = [0u32; W];
    for y in 0..STATUS_BAR_HEIGHT {
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = get_pixel(x, y, app);
        }
        let _ = std::surface_write(surface, y * W, &row);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[AGENT_MGR (Ring 3)] Agent Manager online.").ok();

    // WHY before the app is built, and before a surface exists: building the app
    // builds its body -- the cards are placed, a wide line's rows are wrapped and
    // the scroll range is solved, all from the rung's boxes -- and the first
    // frame is painted from those numbers. A rung published after that would size
    // the ink of the next frame against a body wrapped for the last one, which is
    // the clipped-text defect this file exists to close.
    publish_text_scale();

    let mut app = AgentManagerApp::new();

    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(surface) => surface,
        Err(_) => {
            // WHY it refuses rather than running headless: a supervisor with
            // no surface has nobody to supervise for, and a silent exit would
            // look like a clean start.
            std::log("[AGENT_MGR] Display surface unavailable; refusing headless execution.").ok();
            std::exit_with_code(1);
        }
    };

    refresh_clock(&mut app);
    repaint(surface, &app);
    let _ = std::surface_flip();

    loop {
        // WHY on every wake and not once at launch: the person changes the text
        // size in Settings while this screen is alive. Reading it here is one
        // settings query per wake, ahead of the work this loop does -- never one
        // per glyph, which is the cost the note beside `publish_text_scale`
        // refuses to pay.
        let rung_moved = publish_text_scale();
        if rung_moved {
            // A moved rung is a different wrapping: the cards are placed, a wide
            // line's rows are broken and the scroll range is solved from the
            // rung's boxes, so the body is re-laid before anything measures or
            // paints it this wake.
            app.rebuild();
        }
        // WHY the pipeline is drained before the wait, and not only after a
        // touch woke the app: a key press produces no touch event, so an
        // app that waits for a touch before it reads the pipeline never
        // sees a key until somebody touches the screen. The arrow keys
        // were dead for that reason.
        let mut input = std::InputEvent {
            kind: 0,
            x: 0,
            y: 0,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        };
        let mut dirty = if rung_moved { Dirty::All } else { Dirty::None };
        let mut handled = false;
        while std::input_event_poll(&mut input).is_ok() {
            handled = true;
            if let Some(key) = input.key_event() {
                if key.pressed {
                    // WHY the keys go to the composer only on the chat
                    // surface: on every other surface a key press is the
                    // platform's, and swallowing it would make the app look
                    // broken.
                    if app.surface == Surface::Chat {
                        match handle_chat_key(&mut app, key.code) {
                            ChatKey::Edited => dirty = Dirty::All,
                            ChatKey::Ignored => {}
                        }
                    } else if handle_scroll_key(&mut app, key.code) {
                        dirty = Dirty::All;
                    }
                }
                continue;
            }
            if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
            {
                // A drag over the body scrolls it; anything else is the
                // system's navigation gesture.
                //
                // WHY the body keeps the gesture: the body is a scrollable
                // list, and a list that cannot be scrolled is a list whose
                // lower half cannot be reached.
                let over_body =
                    (input.y as usize) >= body_top() && (input.y as usize) < app.body_bottom();
                if (input.kind == 4 || input.kind == 10) && over_body {
                    app.scroll.apply_drag(input.dy as i32);
                    dirty = Dirty::All;
                    continue;
                }
                std::log("[AGENT_MGR] Navigation gesture recognized. Returning to the launcher.").ok();
                let _ = std::surface_destroy(surface);
                std::exit_with_code(0);
            }
            if input.kind == 1 || input.kind == 2 {
                match handle_tap(&mut app, input.x, input.y) {
                    Tap::Leave(message) => {
                        std::log(message).ok();
                        let _ = std::surface_destroy(surface);
                        std::exit_with_code(0);
                    }
                    Tap::Redraw => dirty = Dirty::All,
                    Tap::None => {}
                }
            }
        }
        if app.chat.is_streaming {
            if app.step_streaming() {
                dirty = Dirty::All;
            }
        } else if app.install_deadline != 0 {
            // WHY the install waits beside the prompt: the platform answers an
            // install when the file has reached the model namespace, and a screen
            // that slept through the wait would show a file it had just been told
            // about as though nothing had happened.
            if app.step_install_reply() {
                dirty = Dirty::All;
            }
        } else if app.probe.state == ProbeState::Awaiting {
            // WHY the probe waits in the same place: the embedding answer is the
            // platform's, and an app that painted its card before the reply landed
            // would say "nothing asked yet" while an answer was one poll away.
            if app.step_embedding_reply() {
                dirty = Dirty::All;
            }
        } else if !handled {
            // Nothing was waiting: sleep until something arrives.
            let _ = std::event_poll(std::EVENT_TOUCH_INPUT, 100);
        }
        // WHY the clock is read here and not in the rasteriser: the band is a
        // few glyphs, the read is a syscall, and the screen is 720x1440 pixels.
        let clock_moved = refresh_clock(&mut app);
        if dirty == Dirty::All {
            repaint(surface, &app);
            let _ = std::surface_flip();
        } else if clock_moved {
            repaint_band(surface, &app);
            let _ = std::surface_flip();
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut msg = [0u8; 128];
    let mut n = 0;
    for &b in b"[PANIC] Agent Manager: " {
        msg[n] = b;
        n += 1;
    }
    if let Some(loc) = info.location() {
        let f = loc.file().as_bytes();
        let flen = f.len().min(40);
        msg[n..n + flen].copy_from_slice(&f[..flen]);
        n += flen;
        msg[n] = b':';
        n += 1;
        n += agent_manager::format_u32(loc.line(), &mut msg[n..]);
    }
    if let Ok(s) = core::str::from_utf8(&msg[..n]) {
        std::log(s).ok();
    } else {
        std::log("[PANIC] Agent Manager app error!").ok();
    }
    std::exit_with_code(1);
}
