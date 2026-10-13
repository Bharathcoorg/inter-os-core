//! Activity Space: contextual workspaces plus the honest running deck.
//!
//! WHAT Activity Space IS (INTEROS_UI_DESIGN_SYSTEM.md section 4 and the
//! `04_activity_space` reference sheet): a defining InterOS concept, a set of
//! *contextual workspaces* -- Work, Personal, Research, Media -- each showing its
//! own apps, tasks and context. It "should feel different from a conventional
//! recent-apps screen". This surface shows both halves of that idea, each only
//! as true as the platform allows:
//!
//!   * The Activities block (bottom): the OS's four defined activity *types*.
//!     These names are fixed by the design system -- the same fixed-category
//!     pattern the app drawer uses for its Work/Personal/Tools tabs -- so they
//!     are the OS's structure, not per-user data. Honest limit, stated plainly
//!     on the surface: this guest has no activity or workspace store, so no
//!     per-workspace task, file, or membership record exists *anywhere* to read.
//!     A workspace therefore reports the one derived fact it can source -- the
//!     number of **installed apps whose package-registry category equals the
//!     activity name**, from the same `state.app_categories` the app drawer
//!     filters -- and never a task or file count, because nothing records them.
//!     Work and Personal resolve to real counts; Research and Media resolve to
//!     an honest zero because no package declares those categories.
//!
//!   * The running deck (top): the recent-apps behaviour a swipe-up opens, kept
//!     intact. `refresh_runtime_recents` copies package ids and kernel task ids
//!     out of `app_runtime_list` into three recent slots; card `i` is slot `i`,
//!     its title is that package id resolved through the registry, its icon is
//!     the pictogram that id maps to, and its subtitle reports whether the
//!     kernel still lists its task. The status-band count is the number of cards
//!     whose subtitle reads "Running", so the deck and the count cannot disagree.
//!     `std::window_list` was considered and is not a source: it reads the
//!     compositor's surface registry, not the task list, and is not on state.
//!
//! WHY the running deck keeps the top band and the workspaces live below it:
//! the shell's ActivitySpace input handler (main.rs, which this file cannot
//! change) hard-codes the three deck cards' tap and swipe bands, and the
//! lifecycle harness taps (360, 340) / (610, 230) and swipes (200,660)->(550,660)
//! against them. The Activities block lies entirely outside those bands, so it is
//! paint-only state, never a control: nothing there answers a touch, because the
//! only input path for this view is the frozen handler and it reaches none of it.
//! A "+ New Activity" action is real only where there is a workspace to add to;
//! with no store and no `settings_set` authority in the launcher's capability
//! table (kernel grants it FileSystemRead only), it is drawn as an explicitly
//! unavailable action with that reason rather than a button that writes nothing.
//!
//! WHAT is never drawn, and why: no per-app strip, no task count, no file count.
//! The screen those replaced named four workspaces and drew thirteen app
//! identities and a "4 apps - 2 tasks" provenance-free subtitle for each, none of
//! it read from anywhere -- the same defect as a clock drawn without an RTC read.
//!
//! WHY `pub`: the shell's ActivitySpace tap handler must ask this screen where
//! its close controls are ([`activity_close_hit`]) rather than carry a second
//! copy of the coordinates, which is how the control a user sees and the region
//! a touch acts on drift apart. [`activity_close_slot`] is the other half of that
//! contract: the one function on this screen that terminates a task.

#![allow(dead_code)]

use crate::{test_text, test_text_aa, LauncherState};
use launcher::icons::{self, IconKind};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, MIN_TOUCH_TARGET, SCREEN_HEIGHT, SCREEN_WIDTH,
    STATUS_BAR_HEIGHT,
};

// -----------------------------------------------------------------------------
// Constants & Colors
// -----------------------------------------------------------------------------

const BG_COLOR: u32 = 0xFF111214;
const CARD_BG: u32 = 0xFF25282A;
const CARD_BORDER: u32 = 0xFF3A3D40;
const TEXT_WHITE: u32 = 0xFFF7F6F2;
const TEXT_MUTED: u32 = 0xFF777773;

const CARD_X: usize = 32;
const CARD_W: usize = SCREEN_WIDTH - 64; // 656

/// Corner radius of a card.
///
/// WHY a named constant and not a `28` at each of the six call sites: the card
/// outline, the card fill mask and the title/subtitle layout all have to agree on
/// it, and the old code typed it three times.
const CARD_RADIUS: usize = 28;

// Card rectangles. WHY these four boxes stay exactly where they are: the shell's
// ActivitySpace tap handler hardcodes bands derived from them, and the lifecycle
// harness taps (610, 230) and swipes across (200, 660) -> (550, 660) to dismiss.
// Resizing a card would move the drawn dismiss badge away from the region the
// tap acts on, which is the exact drift this file exists to stop. Only card 3's
// contents changed; its rectangle is unchanged so nothing above it moved.
//
// Card 0 (recent slot 0): 120 .. 390  (h=270, contains y=230 dismiss badge)
// Card 1 (recent slot 1): 420 .. 690  (h=270, contains y=550 dismiss badge)
// Card 2 (recent slot 2): 720 .. 990  (h=270, contains y=870 dismiss badge)
// Card 3:                  1020 .. 1200 (no recent slot exists to back it)
pub const WORKSPACE_CARDS: [Rect; 4] = [
    Rect::new(CARD_X, 120, CARD_W, 270),
    Rect::new(CARD_X, 420, CARD_W, 270),
    Rect::new(CARD_X, 720, CARD_W, 270),
    Rect::new(CARD_X, 1020, CARD_W, 180),
];

/// Rows a dismissed or closed recent card can move: the running-count strip
/// above the deck, and every card in the deck below it.
///
/// WHY the whole deck and not just the dismissed card: `dismiss_recent` shifts
/// the remaining slots up, so card 0 takes card 1's package, card 1 takes card
/// 2's, and card 2 empties. A band naming one card would leave two stale.
///
/// WHY the count strip is inside the band: `running_count` reads the task ids a
/// close just cleared, so the line above the deck moves with the cards.
///
/// WHY index 2 and not the last card: `WORKSPACE_CARDS` has four entries and
/// `build_card` answers `None` for slot 3, because there are only three recent
/// slots. Card 3 is therefore never drawn and cannot move.
pub fn recents_band() -> (usize, usize) {
    let last = WORKSPACE_CARDS[2];
    (STATUS_BAND_TOP, last.y + last.h)
}

/// Left inset of a card's text column, measured from the card's left edge.
///
/// WHY 96 and not the icon's own 24: the icon is 56 wide, and 96 leaves a 16 px
/// (`space::MD`) gap after it. The old title and subtitle were already at 96, so
/// this is where the eye read a card's identity and moving it would move text
/// the lifecycle harness has been reading at those coordinates.
const CARD_TEXT_X: usize = 96;
/// Vertical offset of a card's title from the card's top edge.
const CARD_TITLE_Y: usize = 26;
/// Vertical offset of a card's subtitle: one line below the title.
const CARD_SUBTITLE_Y: usize = 54;

/// Side and corner radius of a card's app icon.
///
/// WHY 56 and the 14 the old workspace squircle used: the box is no longer a
/// workspace badge, it is the app's own pictogram, and it has to sit in the same
/// 24 px content inset every other control on this card uses. The radius is
/// unchanged so the icon column does not move.
const CARD_ICON_SIZE: usize = 56;
const CARD_ICON_RADIUS: usize = 14;

/// Fill of a card's app icon, and the ink of its outline.
///
/// WHY one neutral slate and not the old four workspace colours (amber, slate,
/// sapphire, magenta): those four were keyed to which apps the card claimed to
/// hold, which is the claim with no source. A colour chosen only to sit on
/// `CARD_BG` asserts nothing, so one colour is correct for every app.
const CARD_ICON_BG: u32 = 0xFF334155;
const CARD_ICON_BORDER: u32 = 0xFF475569;

// Dismiss badges: 48x48 rounded circles centered at test coordinates
// Card 0: center (610, 230) -> x: 586..634, y: 206..254
// Card 1: center (610, 550) -> x: 586..634, y: 526..574
// Card 2: center (610, 870) -> x: 586..634, y: 846..894
const BADGE_SIZE: usize = 48;
/// The badge is a full circle, as the corner close control beside it is.
///
/// WHY its own constant and not `CLOSE_RADIUS`: the two controls happen to be the
/// same size today, but they answer different taps -- one is the shell's
/// swipe-and-tap dismiss band, the other is the hit test exported below -- so
/// changing one must not silently reshape the other.
const BADGE_RADIUS: usize = BADGE_SIZE / 2;
const BADGE_X: usize = 586;
const CARD0_DISMISS: Rect = Rect::new(BADGE_X, 206, BADGE_SIZE, BADGE_SIZE);
const CARD1_DISMISS: Rect = Rect::new(BADGE_X, 526, BADGE_SIZE, BADGE_SIZE);
const CARD2_DISMISS: Rect = Rect::new(BADGE_X, 846, BADGE_SIZE, BADGE_SIZE);

/// The bar under the deck, centered, spanning y in 1220..1264.
///
/// WHY it is the home bar and not the close bar: the shell's ActivitySpace tap
/// handler sends this band to `ViewMode::HomeScreen` and logs "Returned to home
/// grid from recents." -- it has never closed anything. The label read "Close",
/// which contradicted that behaviour and, after the per-card close controls landed
/// below it, implied a control that stopped every app at once and did not. The
/// decision on this screen was not to change navigation to suit a label, so the
/// label changed to match the navigation.
const NAV_HOME_BTN: Rect = Rect::new(SCREEN_WIDTH / 2 - 100, 1220, 200, 44);

const HOME_PILL: Rect = Rect::new(SCREEN_WIDTH / 2 - 90, SCREEN_HEIGHT - 20, 180, 5);

// -----------------------------------------------------------------------------
// Per-card close control
//
// WHY this section exists: before it, the only way to stop an app from this
// screen was the shell's own hardcoded tap bands. The painter and the tap
// handler each carried their own copy of the coordinates, so the control on
// screen and the region a touch acted on were two numbers that had to be kept in
// step by hand -- and card 3 had no close affordance at all. Everything below
// derives the drawn rectangle from one function, and the hit test asks that same
// function, so a tap can only land where the pixels are.
// -----------------------------------------------------------------------------

/// Side of a card's corner close control.
///
/// WHY `MIN_TOUCH_TARGET` (48) and not the 44 the platform minimums suggest: this
/// shell publishes exactly one touch-target token and every other control here is
/// built from it, including the dismiss badge this sits beside. A second, smaller
/// number on one screen is how two surfaces end up disagreeing about how big a
/// finger target has to be. Nothing about the size changes from the old badge --
/// only where the control sits on the card.
const CLOSE_SIZE: usize = MIN_TOUCH_TARGET;

/// Gap between a card's top-right corner and its close control.
///
/// WHY `space::MD` (16): the control floats in the corner rather than sitting in a
/// content column, so the number that matters is where its 48 px circle lands
/// against the card's 28 px corner arc. At 16 the circle's centre is 16 px in and
/// 24 px down from the corner, which is where Android 12 puts its dismiss dot on a
/// card this wide; a smaller inset would set the circle on the corner arc itself.
/// It is a spacing token rather than a number typed here so the inset moves with
/// the scale.
const CLOSE_INSET: usize = space::MD;

/// The close control is a full circle, Android 12 style.
///
/// WHY half the side and not `radius::CHIP`: this is a corner affordance, and a
/// 10 px radius on a 48 px square reads as a chip the user might scroll rather
/// than as the dismiss dot every mobile OS puts in a card corner.
const CLOSE_RADIUS: usize = CLOSE_SIZE / 2;

/// Colours of the close control. WHY local constants rather than theme fields:
/// this screen is already drawn in a fixed dark palette (see `CARD_BG` and
/// friends above), and reaching into the light theme for one control would put
/// two palettes on one surface.
const CLOSE_FILL: u32 = 0xFF2B303A;
const CLOSE_BORDER: u32 = 0xFF4B5563;
/// Ink of a live close mark, and of the "nothing to close" dash that replaces it.
const CLOSE_INK: u32 = 0xFFE5E7EB;
const CLOSE_DISABLED_INK: u32 = 0xFF8B8F96;

/// Half-length of each arm of the close mark, and its half-thickness, in pixels.
///
/// WHY these are the X's own numbers and not a bitmap: `launcher::icons` has no
/// close or dismiss pictogram -- `IconKind` runs Phone..Briefcase and stops at
/// `Briefcase` -- so there is no glyph to ask for. The mark is evaluated as
/// vector geometry from the centre of the same rectangle [`close_control_for`]
/// hands the hit test, which is what the dismiss badge already did; inventing an
/// `IconKind::Close` here would have meant editing `icons.rs`, which this screen
/// does not own.
const CLOSE_ARM: i32 = 11;
const CLOSE_STROKE: i32 = 1;

/// Half-width and half-height of the dash drawn in place of the close mark when
/// the card holds no running task.
///
/// WHY a dash and not a dimmed X: a dimmed X still promises "press this to close
/// something", and pressing it would do nothing. The centred dash is the same
/// "no reading" mark the home surface draws in its fuel gauge (`home.rs`), so
/// the two surfaces cannot disagree about what a dash means.
const CLOSE_DASH_HALF_W: i32 = 7;
const CLOSE_DASH_HALF_H: i32 = 1;

/// One corner close control: where it is, and what it would stop.
///
/// WHY the task id is carried here rather than looked up again by the painter:
/// "this control has nothing to act on" is a property of the frame, so resolving
/// it once per frame keeps the per-pixel path to a comparison.
struct CloseControl {
    rect: Rect,
    /// The kernel task id this control stops. `0` means the card holds no running
    /// task, and the control draws its dash instead of a mark.
    task: u64,
}

/// Where a card's close control sits: its own top-right corner.
fn close_rect(card: Rect) -> Rect {
    Rect::new(
        card.x + card.w - CLOSE_SIZE - CLOSE_INSET,
        card.y + CLOSE_INSET,
        CLOSE_SIZE,
        CLOSE_SIZE,
    )
}

/// The close control drawn on `card` (which is `WORKSPACE_CARDS[slot]`), or
/// `None` when this screen has no recent slot behind that card.
///
/// WHY `None` rather than a disabled button: `LauncherState::recent_task_ids`
/// holds three slots and this screen draws four cards, so the fourth card has no
/// task behind it and nothing a control there could ever stop. The old screen
/// simply omitted the dismiss badge from card 3 and left the shell to discover
/// the gap; drawing a control that cannot act is the dead control this screen
/// used to be full of, so nothing is drawn instead.
fn close_control_for(state: &LauncherState, slot: usize) -> Option<CloseControl> {
    let card = *WORKSPACE_CARDS.get(slot)?;
    let task = *state.recent_task_ids.get(slot)?;
    Some(CloseControl {
        rect: close_rect(card),
        task,
    })
}

/// The close mark, if (`x`, `y`) falls on one arm of the cross in `control`.
fn close_mark_pixel(control: &CloseControl, x: usize, y: usize) -> Option<u32> {
    let dx = x as i32 - control.rect.center_x() as i32;
    let dy = y as i32 - control.rect.center_y() as i32;
    if dx.abs() > CLOSE_ARM || dy.abs() > CLOSE_ARM {
        return None;
    }
    // The two diagonals, each one stroke wide either side of its own line.
    if (dx - dy).abs() <= CLOSE_STROKE || (dx + dy).abs() <= CLOSE_STROKE {
        Some(CLOSE_INK)
    } else {
        None
    }
}

/// The "nothing to close" dash, if (`x`, `y`) falls on it.
fn close_dash_pixel(control: &CloseControl, x: usize, y: usize) -> Option<u32> {
    let dx = (x as i32 - control.rect.center_x() as i32).abs();
    let dy = (y as i32 - control.rect.center_y() as i32).abs();
    if dx <= CLOSE_DASH_HALF_W && dy <= CLOSE_DASH_HALF_H {
        Some(CLOSE_DISABLED_INK)
    } else {
        None
    }
}

/// How many of the shell's recent slots hold a live task.
///
/// WHY `task_id != 0` counts as a live task: `refresh_runtime_recents` in the
/// shell fills those slots from `app_runtime_list`, so a non-zero id is a task
/// the kernel listed on its last query and a zero is "this slot holds nothing".
/// It is not the shell guessing from the recent-apps stack, which survives apps
/// that have already exited.
///
/// WHY this is the same number the deck draws as "Running": [`build_card`] reads
/// `task != 0` from the identical field, so the status line and the cards cannot
/// disagree about what is running.
fn running_count(state: &LauncherState) -> usize {
    let mut n = 0usize;
    for task in state.recent_task_ids.iter() {
        if *task != 0 {
            n += 1;
        }
    }
    n
}

// -----------------------------------------------------------------------------
// Status band
// -----------------------------------------------------------------------------

/// Top of the strip between the status bar and the first card.
///
/// WHY `STATUS_BAR_HEIGHT + 12`: `get_activity_color` answers everything above
/// that line with plain background, so this is where this surface can print
/// without moving a pixel of the deck. `WORKSPACE_CARDS[0]` begins at y 120, which
/// is the other end of the strip.
const STATUS_BAND_TOP: usize = STATUS_BAR_HEIGHT + 12;

/// Height of that strip, taken from the deck rather than retyped.
const STATUS_BAND_H: usize = WORKSPACE_CARDS[0].y - STATUS_BAND_TOP;

/// Byte capacity of one status line.
///
/// WHY 48 and not a tighter bound: the widest line below is the running count at
/// 28 bytes plus its two-digit cap, and a buffer that is only just big enough is
/// a buffer that silently truncates the next line someone adds.
const STATUS_CAP: usize = 48;

/// The screen's two honest status lines.
///
/// WHY two CAPTION lines and not a larger single headline: the strip is 60 px
/// tall, `type_scale::CAPTION` is the screen's own card-title size, and two of
/// them are 48 px -- the only two-line treatment that fits without running into
/// `WORKSPACE_CARDS[0]`. A single `BODY` headline would fit, but it would leave
/// the reason there is nothing to close unsaid, which is the half of the message
/// a user actually needs.
const EMPTY_HEAD: &[u8] = b"No running apps";
const EMPTY_BODY: &[u8] = b"The task list is empty - nothing to close";

/// Copies `src` into `buf` at `at`, stopping at the end; returns the new end.
fn put(buf: &mut [u8], at: usize, src: &[u8]) -> usize {
    let mut end = at;
    for b in src {
        if end >= buf.len() {
            break;
        }
        buf[end] = *b;
        end += 1;
    }
    end
}

/// Writes `value` in decimal into `buf` at `at`; returns the new end.
fn push_decimal(buf: &mut [u8], at: usize, value: usize) -> usize {
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut v = value;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut end = at;
    for i in (0..n).rev() {
        end = put(buf, end, &digits[i..i + 1]);
    }
    end
}

/// "N running - X closes an app", with `n` counted from the kernel's own task list.
fn running_line(buf: &mut [u8; STATUS_CAP], n: usize) -> &[u8] {
    let mut at = push_decimal(buf, 0, n);
    at = put(buf, at, b" running - X closes an app");
    &buf[..at]
}

// -----------------------------------------------------------------------------
// Activity Space workspaces (contextual activities)
//
// The design system defines Activity Space as contextual workspaces. This part
// of the screen draws them. Everything below is either a design token or a value
// derived from one real source, and the source is named on the surface.
// -----------------------------------------------------------------------------

/// Colours for the workspaces block, taken from the theme the whole surface
/// already uses rather than retyped as hex -- the same rule the rest of the shell
/// follows. This surface is the design system's dark treatment.
const WS_BG: u32 = Theme::DARK.bg;
const WS_CARD: u32 = Theme::DARK.surface;
/// Recessed fill of the "+ New Activity" affordance: it is an unavailable action,
/// so it must not wear the raised fill of a live control.
const WS_CARD_INERT: u32 = Theme::DARK.surface_sunken;
const WS_BORDER: u32 = Theme::DARK.border;
const WS_TEXT: u32 = Theme::DARK.text;
const WS_MUTED: u32 = Theme::DARK.text_muted;

/// The four contextual activity types, from INTEROS_UI_DESIGN_SYSTEM.md section 4
/// and the `04_activity_space` reference sheet.
///
/// WHY these four names are not fabrication: they are the OS's defined activity
/// structure -- the same fixed-category pattern as the app drawer's
/// Work/Personal/Tools tabs (`APP_DRAWER_TAB_NAMES`), named by the system rather
/// than chosen per user. What *would* be fabrication is inventing each one's
/// contents; the contents below are derived from the registry and stated as such.
pub const ACTIVITY_NAMES: [&[u8]; 4] = [b"Work", b"Personal", b"Research", b"Media"];

/// Top of the "Activities" heading, just below the running deck (which ends at
/// WORKSPACE_CARDS[2].y = 990). The whole workspaces block lives below the
/// deck because the shell pins the deck to the top for input.
const ACTIVITY_HEAD_Y: usize = 1000;
/// Top of the one-line honesty note that fixes what each per-workspace count means.
const ACTIVITY_SUB_Y: usize = 1046;
/// The workspaces rows align with the deck's own content columns, so the two
/// blocks read as one screen rather than two stitched together.
/// Which workspace row a tap at (`x`, `y`) falls in, if any.
///
/// WHY this exists: the Activities block was paint-only — `main.rs`'s
/// `ViewMode::ActivitySpace` arm reached only the recents deck's three pinned
/// bands and the home bar, so nothing below the deck answered to a touch. This
/// is the hit test that makes it answer.
///
/// The band is `ACTIVITY_ROW_TOP .. + 4*ACTIVITY_ROW_STRIDE` across the full
/// card column, which is below the last recents card (`WORKSPACE_CARDS[2]` ends
/// at row 990) and above the home bar. That is what keeps it clear of every band
/// the layout harness pins, so adding it cannot steal a tap the harness expects
/// elsewhere.
///
/// A row is `ACTIVITY_ROW_H` tall inside a `ACTIVITY_ROW_STRIDE` pitch, so a tap
/// in the gap between two rows is not a tap on either — that is what the modulo
/// below is for, and it is why a gap tap falls through to the deck rather than
/// opening a row the user did not aim at.
pub fn activity_row_at(x: usize, y: usize) -> Option<usize> {
    let top = ACTIVITY_ROW_TOP;
    if y < top || y >= top + ACTIVITY_NAMES.len() * ACTIVITY_ROW_STRIDE {
        return None;
    }
    let offset = y - top;
    if offset % ACTIVITY_ROW_STRIDE >= ACTIVITY_ROW_H {
        return None;
    }
    if x < ACTIVITY_ROW_X || x - ACTIVITY_ROW_X >= ACTIVITY_ROW_W {
        return None;
    }
    Some(offset / ACTIVITY_ROW_STRIDE)
}

pub const ACTIVITY_ROW_X: usize = CARD_X;
const ACTIVITY_ROW_W: usize = CARD_W;
const ACTIVITY_ROW_H: usize = 32;
const ACTIVITY_ROW_STRIDE: usize = 34;
const ACTIVITY_ROW_TOP: usize = 1076;
const ACTIVITY_RADIUS: usize = radius::CHIP;

/// "+ New Activity" availability control, below the home bar with its reason.
const NEW_ACTION_W: usize = 260;
const NEW_ACTION_H: usize = 48;
const NEW_ACTION_X: usize = (SCREEN_WIDTH - NEW_ACTION_W) / 2;
const NEW_ACTION_Y: usize = 1330;
const NEW_NOTE_A_Y: usize = 1290;
const NEW_REASON_Y: usize = 1384;

/// Honest strings. Each one names where a number came from or why a thing is
/// absent, so the screen never shows a plausible value with no source.
const ACTIVITY_SUB: &[u8] = b"Installed apps per registry category.";
const ACTIVITY_NOTE_A: &[u8] = b"No per-workspace task, file or membership is recorded.";
const ACTIVITY_REASON: &[u8] = b"Unavailable: the shell cannot persist a workspace.";

/// How many installed apps carry the package-registry category equal to
/// `activity`.
///
/// WHY this and not a workspace store: none exists. The app drawer already
/// filters `state.app_categories` with this exact `cat == b"Work"` comparison
/// (`apps/launcher/src/screens/app_drawer.rs`), so this is the one per-app
/// grouping the platform records, and reporting its count for an activity of the
/// same name is honest *with the source stated* (see [`ACTIVITY_SUB`]). It is not
/// activity membership, which is why the surface never calls it that. Work and
/// Personal match real categories; Research and Media match none, so they
/// honestly read zero -- exactly like the drawer's "No Work Apps" empty state.
fn activity_app_count(state: &LauncherState, activity: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while i < state.app_count.min(32) {
        let l = state.app_category_lens[i] as usize;
        if &state.app_categories[i][..l] == activity {
            n += 1;
        }
        i += 1;
    }
    n
}

/// "N apps" for the count cell. `n` is at most the 32-app catalogue, so a
/// two-digit decimal is exact and the 16-byte buffer never truncates.
fn activity_count_text(n: usize) -> ([u8; 16], usize) {
    let mut buf = [0u8; 16];
    let mut len = 0usize;
    if n >= 10 {
        buf[len] = b'0' + (n / 10) as u8;
        len += 1;
    }
    buf[len] = b'0' + (n % 10) as u8;
    len += 1;
    let mut j = 0usize;
    while j < b" apps".len() {
        buf[len] = b" apps"[j];
        len += 1;
        j += 1;
    }
    (buf, len)
}

/// One activity row: its card box, its name, and its honest registry-derived
/// count, right-aligned so the counts read as a column.
#[derive(Clone, Copy)]
struct WorkspaceRow {
    rect: Rect,
    name: LabelRun,
    count: LabelRun,
}

/// The whole workspaces block, resolved once per frame.
struct WorkspacePanel {
    heading: LabelRun,
    sub: LabelRun,
    rows: [WorkspaceRow; 4],
    new_rect: Rect,
    new_label: LabelRun,
    note_a: LabelRun,
    reason: LabelRun,
}

/// Resolves activity row `i` from the registry: the design type's name on the
/// left and, on the right, how many installed apps the registry categorises under
/// that name. No task or file count is computed, because nothing holds one.
fn build_activity_row(state: &LauncherState, i: usize, scale: usize) -> WorkspaceRow {
    let ry = ACTIVITY_ROW_TOP + i * ACTIVITY_ROW_STRIDE;
    let rect = Rect::new(ACTIVITY_ROW_X, ry, ACTIVITY_ROW_W, ACTIVITY_ROW_H);
    // WHY the same (h - line)/2 centring the nav label uses: it keeps a change of
    // type scale from sliding the name off the row box.
    let text_y = ry + ACTIVITY_ROW_H.saturating_sub(text::line_height(scale)) / 2;
    let name = LabelRun::new(rect.x + 16, text_y, scale, ACTIVITY_NAMES[i], WS_TEXT);
    let (cnt, cnt_len) = activity_count_text(activity_app_count(state, ACTIVITY_NAMES[i]));
    let cnt_w = text::measure(&cnt[..cnt_len], scale);
    let count = LabelRun::new(
        rect.x + rect.w - 16 - cnt_w,
        text_y,
        scale,
        &cnt[..cnt_len],
        WS_MUTED,
    );
    WorkspaceRow { rect, name, count }
}

// -----------------------------------------------------------------------------
// Rounded Rectangle Helpers
// -----------------------------------------------------------------------------

#[inline]
fn rounded_contains(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    if !r.contains(x, y) {
        return false;
    }
    let rad = rad.min(r.w / 2).min(r.h / 2);
    if rad == 0 {
        return true;
    }
    let left = r.x + rad;
    let right = r.x + r.w - 1 - rad;
    let top = r.y + rad;
    let bottom = r.y + r.h - 1 - rad;
    if (x >= left && x <= right) || (y >= top && y <= bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rad as i32) * (rad as i32)
}

#[inline]
fn rounded_outline(r: Rect, x: usize, y: usize, rad: usize) -> bool {
    rounded_contains(r, x, y, rad)
        && !rounded_contains(r.inset(1), x, y, rad.saturating_sub(1))
}

// -----------------------------------------------------------------------------
// Text Run Handling
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer.
///
/// WHY 96 and not the old 64: `text::ellipsize` writes into a 96-byte buffer, so
/// a run buffer smaller than that would have to copy at most that many bytes back
/// and would silently shorten a fitted label after measuring the whole thing.
const RUN_CAP: usize = 96;

/// The three numbers a run needs before it can be gated on both faces' extents:
/// the em to rasterise it at, and the top row and height of the union box.
///
/// WHY one function and not arithmetic at each constructor: the box a run is
/// tested against has to be the union of the bitmap cell and the outline line
/// box, and the two are not the same shape -- at a scale-1 caption the outline's
/// descent reaches 6 rows below the run's baseline while the bitmap's own
/// descender stops at row 23 of a 24-row cell, so a box measured for the cell
/// alone clips a descender off the bottom of the text. This is the same
/// derivation `home.rs` uses, and it is why the box is derived rather than
/// inherited from `text::line_height`.
fn activity_run_metrics(y: usize, scale: usize) -> (u32, usize, usize) {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = y + launcher::font::BASELINE * scale;
    let top = baseline.saturating_sub(asc as usize);
    (em, top, (baseline + desc as usize).saturating_sub(top))
}

#[derive(Clone, Copy)]
struct LabelRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    /// Pixels per em for this run's outline rendering.
    em: u32,
    /// Top row of the box covering BOTH renderers, not the bitmap cell's.
    top: usize,
    /// Height of that union box.
    box_h: usize,
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
        em: 0,
        top: 0,
        box_h: 0,
        bytes: [0u8; RUN_CAP],
    };

    fn new(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        // WHY the width is the wider of the two measures: the outline face is
        // condensed but is not always the narrower of the two -- "No running
        // apps" measures 146 bitmap advances at scale 1 and a 146px outline
        // advance here, while "3 running - X closes an app" is 240 at scale 1
        // against a 240px outline advance; the union is taken because the other
        // direction also happens on this surface's vocabulary.
        let w = text::measure(&buf[..len], scale)
            .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize);
        LabelRun {
            x,
            y,
            w,
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    /// `bytes` at (`x`, `y`), shortened with an ellipsis if wider than `fit`.
    ///
    /// WHY a card name needs this and the old hardcoded ones did not: a card is
    /// named from the package registry now, and a registry label runs to sixteen
    /// bytes, so a long name would otherwise run under the close control. The box
    /// below is measured from the run that is actually drawn, not from the run
    /// that was offered, so a shortened name still inks whole.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let len = len.min(RUN_CAP);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x,
            y,
            w: text::measure(&buf[..len], scale)
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    fn centred(x: usize, y: usize, w: usize, scale: usize, bytes: &[u8], ink: u32) -> Self {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let measured = text::measure(&buf[..len], scale);
        let inset = text::center_x(&buf[..len], scale, w);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x: x + inset,
            y,
            w: measured
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    /// As [`LabelRun::fitted`], centred across `centre` starting from `x`.
    ///
    /// WHY two widths: a fit is against the space the run may occupy, and the
    /// centring is across the whole box it sits in, so collapsing them into one
    /// argument would move the run off centre whenever it was shortened.
    fn fitted_centred(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Self {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let len = len.min(RUN_CAP);
        let inset = text::center_x(&buf[..len], scale, centre);
        let (em, top, box_h) = activity_run_metrics(y, scale);
        LabelRun {
            x: x + inset,
            y,
            w: text::measure(&buf[..len], scale)
                .max(interos_std::design::typeface::text_advance_px(&buf[..len], em) as usize),
            h: text::line_height(scale),
            scale,
            len,
            ink,
            em,
            top,
            box_h,
            bytes: buf,
        }
    }

    #[inline]
    fn pixel(&self, px: usize, py: usize) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        // Choose the renderer ONCE for the run, not once per pixel. The placement
        // is a pure function of the run's origin, em and bytes, so every pixel of
        // this run reaches the same face. A per-pixel fallback would draw the
        // glyph from both faces at once -- an outline glyph under a hard-edged
        // bitmap ghost.
        if let Some(run) = launcher::outline_text::OutlineRun::place(
            self.x,
            self.y + launcher::font::BASELINE * self.scale,
            self.em,
            &self.bytes[..self.len],
        ) {
            return run.pixel(px, py, self.ink, 0);
        }
        // The bitmap face is the refusal path, not the baseline: a run the
        // rasteriser declines still has to draw as text.
        test_text(
            px,
            py,
            self.x,
            self.y,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
        )
    }

    /// As [`Run::pixel`], anti-aliased against `bg` so a glyph over a flat card
    /// does not show a stair-stepped outline.
    #[inline]
    fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py - self.top >= self.box_h {
            return None;
        }
        // Choose the renderer ONCE for the run, not once per pixel.
        if let Some(run) = launcher::outline_text::OutlineRun::place(
            self.x,
            self.y + launcher::font::BASELINE * self.scale,
            self.em,
            &self.bytes[..self.len],
        ) {
            return run.pixel(px, py, self.ink, bg);
        }
        test_text_aa(
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
// Activity Space Frame Plan
// -----------------------------------------------------------------------------

/// One recent-apps slot, with everything it prints already resolved.
///
/// WHY there is no mini-app strip on a card any more: the strips were thirteen
/// hardcoded app identities (Files, Calculator, Browser, Calendar, Messages,
/// Phone, Contacts, Terminal, Camera, Vault, Music, Gallery) that the shell never
/// read, drawn whether or not those apps were installed. A card's identity is now
/// the one real fact about a task -- the package id the kernel listed -- so there
/// is nothing left for a strip of *other* apps to say, and drawing one would
/// assert that those apps belong to this task, which nothing in the system
/// records.
struct CardPlan {
    rect: Rect,
    icon_rect: Rect,
    /// The app's own pictogram, when its package id maps to a first-party glyph.
    icon: Option<IconKind>,
    /// The registry's icon word, drawn only when there is no pictogram.
    badge: LabelRun,
    title: LabelRun,
    subtitle: LabelRun,
    dismiss_rect: Option<Rect>,
    /// This card's corner close control, or `None` on a card with no recent slot
    /// behind it (see [`close_control_for`]).
    close: Option<CloseControl>,
}

struct ActivityFrame {
    clock: LabelRun,
    /// The screen's one status line: the running count, or the empty state.
    status_head: LabelRun,
    /// The reason printed under `status_head` when there is nothing running.
    status_body: LabelRun,
    /// One entry per card rectangle. `None` is a card with no recent slot behind
    /// it, which draws nothing at all.
    ///
    /// WHY an `Option` per slot rather than four cards that hide their content:
    /// an empty card outline is a shape that asserts a task the shell does not
    /// have, which is what the deck used to claim four times over.
    cards: [Option<CardPlan>; 4],
    /// The contextual-activity block below the deck (paint-only; see module doc).
    workspaces: WorkspacePanel,
    nav_home_btn: Rect,
    nav_home_label: LabelRun,
    home_pill: Rect,
}

// -----------------------------------------------------------------------------
// Where a card's words come from
//
// Every byte printed on a card resolves through the helpers below, out of three
// real sources and nothing else:
//
//   * `state.recent_apps[i]`     -- the package id `app_runtime_list` returned
//   * `state.recent_task_ids[i]` -- the kernel task id that came with it
//   * the package registry       -- the display name and icon word for that id
//
// The old cards had none of these. They printed four workspace names and
// thirteen app names from string literals, and their subtitle counted the
// literals it had just drawn.
// -----------------------------------------------------------------------------

/// Capacity of a card's subtitle line.
///
/// WHY 32 and not the old 24: the running subtitle carries a kernel task id,
/// which is a `u64` and can reach ten digits, so "Running <dot> task " plus ten
/// digits is 25 bytes and the old buffer would have cut the id in half.
const SUB_CAP: usize = 32;

/// Subtitle prefixes for a card whose task the kernel still lists.
///
/// WHY the separator byte `\xb7`: it has no glyph in this face, so it renders as
/// the gap the old "4 apps <dot> 2 tasks" literals showed, without borrowing a
/// punctuation glyph the rest of the shell does not use.
///
/// WHY these two runs stay on the bitmap face: the byte is outside the outline
/// face's encoded range (32..=126, and 0xb7 is 183), so
/// `interos_std::design::typeface::advance_px` returns 0 for it and
/// `OutlineRun::place` refuses the whole run rather than render a glyphless
/// byte as a space. Measured, "Running \xb7 task 12" refuses for exactly this
/// reason, so a running card's subtitle keeps the bitmap face and the same ink
/// it had before; `SUB_NOT_RUNNING` ("Not running") has no such byte and does
/// take the outline face.
const SUB_RUNNING: &[u8] = b"Running \xb7 task ";
const SUB_ACTIVE: &[u8] = b"Running \xb7 active";
/// Subtitle of a recent whose task is gone.
const SUB_NOT_RUNNING: &[u8] = b"Not running";

/// Ink of a card's subtitle: the accent while the kernel lists its task, muted
/// once it does not.
///
/// WHY the two states differ in colour at all: this is exactly the distinction
/// the status band's "N running" counts, so a user scanning the deck can see
/// which cards the count includes without reading every subtitle.
const SUB_INK_RUNNING: u32 = 0xFF38BDF8;

/// The card's subtitle, built from the one fact about it the kernel supplied.
///
/// WHY three shapes and not one: `recent_task_ids[i]` is the id the kernel
/// returned from `app_runtime_list` on the last query and `0` when the slot has no
/// live task, so "Running" and "Not running" are both readings rather than
/// guesses. When the task is also the shell's focused child the line says so,
/// which is the real form of the focus indicator the old cards faked by testing
/// whether the active task's label happened to be in a hardcoded workspace
/// membership list.
fn card_subtitle(buf: &mut [u8; SUB_CAP], task: u64, focused: bool) -> &[u8] {
    if task == 0 {
        return SUB_NOT_RUNNING;
    }
    if focused {
        return SUB_ACTIVE;
    }
    let mut at = put(buf, 0, SUB_RUNNING);
    at = push_decimal(buf, at, task as usize);
    &buf[..at]
}

/// Catalogue entry whose package id is exactly `id`, if the registry names it.
///
/// WHY the id is matched whole and not by suffix: a suffix match would let
/// "org.interos.files" resolve to the entry for "org.interos.file_manager", which
/// is how a card ends up showing one app's name under another's id.
fn registry_index(state: &LauncherState, id: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < state.app_count {
        let len = (state.app_id_lens[i] as usize).min(state.app_ids[i].len());
        if len == id.len() && &state.app_ids[i][..len] == id {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The registry's icon word for entry `index`, trailing spaces trimmed.
///
/// WHY trim rather than slice to a length field: `refresh_package_catalog` pads
/// the field to four bytes, so "SET" arrives as "SET " and drawing all four
/// glyphs paints a space into the middle of the icon box.
fn registry_badge(state: &LauncherState, index: usize) -> &[u8] {
    let mut len = 0usize;
    while len < state.app_icons[index].len() && state.app_icons[index][len] != b' ' {
        len += 1;
    }
    &state.app_icons[index][..len]
}

/// The title a card prints: the registry's display name for the slot's package id,
/// or the id itself when the registry does not name it.
///
/// WHY the id is the fallback rather than a made-up name: an id the registry no
/// longer names is still a real package, and printing `org.interos.x` says
/// exactly that, whereas substituting a friendly word would be inventing a label
/// the system does not hold.
fn card_title_bytes<'a>(state: &'a LauncherState, id: &'a [u8]) -> &'a [u8] {
    match registry_index(state, id) {
        Some(index) => {
            let len = (state.app_label_lens[index] as usize).min(state.app_labels[index].len());
            if len > 0 {
                &state.app_labels[index][..len]
            } else {
                id
            }
        }
        None => id,
    }
}

/// The dismiss badge the shell's tap handler aims at for `slot`, if that card has
/// one.
///
/// WHY a function over four constants reached for by index: the badges sit inside
/// the card rectangles, so naming them by slot keeps a badge travelling with its
/// card, and it puts "which cards have a badge" in one place instead of four
/// scattered `Some(...)` / `None` pairs. The coordinates are unchanged -- they
/// are the ones the lifecycle harness taps.
fn dismiss_rect_for(slot: usize) -> Option<Rect> {
    match slot {
        0 => Some(CARD0_DISMISS),
        1 => Some(CARD1_DISMISS),
        2 => Some(CARD2_DISMISS),
        _ => None,
    }
}

/// Builds card `slot`, or `None` when there is no real task behind it.
///
/// WHY `None` for an empty slot: the alternative is a card with a border and no
/// contents, which is a shape asserting a task the shell does not have.
fn build_card(state: &LauncherState, slot: usize) -> Option<CardPlan> {
    let rect = *WORKSPACE_CARDS.get(slot)?;
    let raw = state.recent_apps.get(slot)?;
    let len = (*state.recent_app_lens.get(slot)? as usize).min(raw.len());
    if len == 0 {
        // WHY this is the only "nothing here" test: a slot with no package id is
        // the one case where the shell has no identity to print. Everything else
        // about a card -- the task, the registry entry, the pictogram -- is read
        // below, and each of those has its own honest rendering when it is absent.
        return None;
    }
    let id = &raw[..len];

    // WHY the icon column is derived from the text column rather than typed: it is
    // one icon width and one gap short of it, so the two can never disagree about
    // where a card's content starts. (96 - 56 - 16 = 24, the inset the old
    // workspace squircle used, so the icon does not move.)
    let icon_rect = Rect::new(
        rect.x + (CARD_TEXT_X - CARD_ICON_SIZE - space::MD),
        rect.y + 24,
        CARD_ICON_SIZE,
        CARD_ICON_SIZE,
    );
    let title_x = rect.x + CARD_TEXT_X;
    // WHY the fit stops at the close control: the control occupies the last
    // `CLOSE_SIZE + CLOSE_INSET` px of the card, so a name running under it would
    // be unreadable exactly where the card's most important control is.
    let title_fit = (close_rect(rect).x - title_x - space::MD).max(1);

    // WHY `copied().unwrap_or(0)`: `recent_task_ids` and `recent_apps` are the same
    // length, so the slot exists by the time this runs -- but a card must not index
    // on that assumption, and a zero is the honest reading of "this slot has no
    // task", which is what the dash and "Not running" are drawn for.
    let task = state.recent_task_ids.get(slot).copied().unwrap_or(0);
    let focused = task != 0 && state.active_child == Some(task);
    let mut sub_buf = [0u8; SUB_CAP];
    let subtitle = LabelRun::new(
        title_x,
        rect.y + CARD_SUBTITLE_Y,
        type_scale::CAPTION,
        card_subtitle(&mut sub_buf, task, focused),
        if task == 0 { TEXT_MUTED } else { SUB_INK_RUNNING },
    );

    // WHY the registry is walked once per card: the title and the badge both need
    // the same entry, and resolving it once keeps a card to a single pass over the
    // catalogue per frame.
    let index = registry_index(state, id);
    let badge_bytes: &[u8] = match index {
        Some(i) => registry_badge(state, i),
        None => &[],
    };
    let badge = if badge_bytes.is_empty() {
        LabelRun::EMPTY
    } else {
        // WHY measured and not offset: the badge is drawn inside the icon box, so
        // it is centred across that box's width and inset by its own real line
        // height -- not by the hardcoded offsets the old workspace badges used.
        LabelRun::fitted_centred(
            icon_rect.x,
            icon_rect.y
                + (icon_rect.h.saturating_sub(text::line_height(type_scale::CAPTION))) / 2,
            type_scale::CAPTION,
            icon_rect.w,
            icon_rect.w,
            badge_bytes,
            TEXT_WHITE,
        )
    };

    Some(CardPlan {
        rect,
        icon_rect,
        icon: icons::match_icon_kind(id),
        badge,
        title: LabelRun::fitted(
            title_x,
            rect.y + CARD_TITLE_Y,
            type_scale::CAPTION,
            title_fit,
            card_title_bytes(state, id),
            TEXT_WHITE,
        ),
        subtitle,
        dismiss_rect: dismiss_rect_for(slot),
        close: close_control_for(state, slot),
    })
}

fn build_activity_frame(state: &LauncherState) -> ActivityFrame {
    // 1. Status Bar Clock
    // WHY empty when there is no reading: the previous fallback drew `b"09:20"`,
    // a plausible-looking time with no RTC read behind it, on the surface a user
    // reads as the state of their device. `LabelRun::new` with an empty run inks
    // nothing, so the clock is simply absent until the kernel answers.
    let clock_slice: &[u8] = if state.clock_time[0] != b' ' {
        &state.clock_time
    } else {
        b""
    };
    let clock = LabelRun::new(48, 14, type_scale::CAPTION, clock_slice, TEXT_WHITE);

    // 2. Cards: one per recent slot, or nothing at all.
    //
    // WHY a list and not four literals: the four hand-written card plans were the
    // home of every fabricated thing on this screen -- four workspace names,
    // thirteen app names in the mini strips, and a subtitle counting the very tiles
    // it had just drawn. Card `i` is now built from recent slot `i` by
    // [`build_card`], which prints only what the kernel's `app_runtime_list` put in
    // that slot, and a slot with nothing in it produces no card.
    //
    // WHY the list still has four entries when only three recent slots exist:
    // `build_card` answers `None` for the fourth, and letting it answer is what
    // keeps "there is no fourth card" in one place rather than in a comment.
    let cards = [
        build_card(state, 0),
        build_card(state, 1),
        build_card(state, 2),
        build_card(state, 3),
    ];

    // 3. Status band: the running count, or the empty state when nothing is
    // running. WHY this is counted from `recent_task_ids` and not from the deck:
    // the deck shows recents, which survive apps that have exited, so the number
    // of cards drawn is not the number running. `running_count` reads the same
    // `task != 0` that each card's subtitle reads, so the two agree by
    // construction rather than by agreement.
    let running = running_count(state);
    let mut status_head_buf = [0u8; STATUS_CAP];
    let head_scale = type_scale::CAPTION;
    let (status_head, status_body) = if running == 0 {
        (
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP,
                CARD_W,
                head_scale,
                EMPTY_HEAD,
                TEXT_WHITE,
            ),
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP + text::line_height(head_scale),
                CARD_W,
                head_scale,
                EMPTY_BODY,
                TEXT_MUTED,
            ),
        )
    } else {
        // WHY centred in the band rather than at its top: with one line there is
        // nothing to stack under it, so the line sits in the middle of the same
        // 60px strip the empty state's first line occupies. `saturating_sub`
        // because a band shorter than one line must clamp to the band's own top,
        // not wrap around and print the line above the status bar.
        (
            LabelRun::centred(
                CARD_X,
                STATUS_BAND_TOP
                    + STATUS_BAND_H.saturating_sub(text::line_height(head_scale)) / 2,
                CARD_W,
                head_scale,
                running_line(&mut status_head_buf, running),
                TEXT_WHITE,
            ),
            LabelRun::EMPTY,
        )
    };

    // 4. Nav bar under the deck. WHY its label is offset by the button's own
    // height less the line's real height rather than by a typed 12: the line box
    // is what `LabelRun` gates on, so centring it against the button's height is
    // what keeps a change of type scale from sliding the label out of the bar.
    let nav_label_scale = type_scale::CAPTION;
    let nav_home_label = LabelRun::centred(
        NAV_HOME_BTN.x,
        NAV_HOME_BTN.y
            + (NAV_HOME_BTN.h.saturating_sub(text::line_height(nav_label_scale))) / 2,
        NAV_HOME_BTN.w,
        nav_label_scale,
        b"Home",
        TEXT_WHITE,
    );

    // 5. Activity workspaces. WHY the counts and not the mock-up's "N apps - M
    // tasks": a task and file count have no source anywhere on this guest, so
    // they are never shown; each row reports the one derived number (installed
    // apps by registry category, matching the app drawer) with the source named
    // by the subheading. WHY "+ New Activity" is inert and recessed, not the
    // raised control the reference sheet shows: there is no workspace store to
    // add to and the launcher holds no `settings_set` authority, so a live
    // button would be a control that writes nothing.
    let row_scale = type_scale::CAPTION;
    let ws_heading =
        LabelRun::new(ACTIVITY_ROW_X, ACTIVITY_HEAD_Y, type_scale::BODY, b"Activities", WS_TEXT);
    let ws_sub = LabelRun::fitted(
        ACTIVITY_ROW_X,
        ACTIVITY_SUB_Y,
        row_scale,
        ACTIVITY_ROW_W,
        ACTIVITY_SUB,
        WS_MUTED,
    );
    let rows = [
        build_activity_row(state, 0, row_scale),
        build_activity_row(state, 1, row_scale),
        build_activity_row(state, 2, row_scale),
        build_activity_row(state, 3, row_scale),
    ];
    let new_rect = Rect::new(NEW_ACTION_X, NEW_ACTION_Y, NEW_ACTION_W, NEW_ACTION_H);
    let new_label = LabelRun::centred(
        new_rect.x,
        new_rect.y + new_rect.h.saturating_sub(text::line_height(row_scale)) / 2,
        new_rect.w,
        row_scale,
        b"+ New Activity",
        WS_MUTED,
    );
    let note_scale = type_scale::CAPTION;
    let ws_note_a = LabelRun::fitted_centred(
        space::XL,
        NEW_NOTE_A_Y,
        note_scale,
        SCREEN_WIDTH - 2 * space::XL,
        SCREEN_WIDTH,
        ACTIVITY_NOTE_A,
        WS_MUTED,
    );
    let ws_reason = LabelRun::fitted_centred(
        space::XL,
        NEW_REASON_Y,
        note_scale,
        SCREEN_WIDTH - 2 * space::XL,
        SCREEN_WIDTH,
        ACTIVITY_REASON,
        WS_MUTED,
    );
    let workspaces = WorkspacePanel {
        heading: ws_heading,
        sub: ws_sub,
        rows,
        new_rect,
        new_label,
        note_a: ws_note_a,
        reason: ws_reason,
    };

    ActivityFrame {
        clock,
        status_head,
        status_body,
        cards,
        workspaces,
        nav_home_btn: NAV_HOME_BTN,
        nav_home_label,
        home_pill: HOME_PILL,
    }
}

// -----------------------------------------------------------------------------
// Per-Pixel Renderer
// -----------------------------------------------------------------------------

static mut FRAME_CACHE: Option<ActivityFrame> = None;

#[inline(never)]
pub fn get_activity_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let cached = unsafe {
        let slot: *mut Option<ActivityFrame> = core::ptr::addr_of_mut!(FRAME_CACHE);
        // WHY (0, 0) is the frame boundary: the shell walks the surface in raster
        // order from the top-left pixel, so (0, 0) is the first pixel of every
        // frame and the only pixel within a frame that repeats it. `LauncherState`
        // is borrowed immutably for the whole frame, so rebuilding there is once per
        // frame and the plan can never lag the state it describes.
        if (x == 0 && y == 0) || (*slot).is_none() {
            *slot = Some(build_activity_frame(state));
        }
        (*slot).as_ref()
    };
    // WHY the background and not a panic: the slot was filled on the line above, so
    // `None` is not reachable through this function. The old code asserted that
    // with `unwrap()`, which meant a state it had not anticipated took the whole
    // shell down over one pixel. If the cache is ever empty here, this pixel
    // honestly has nothing behind it, and the honest colour for "nothing" on this
    // surface is its own background -- the same thing the empty state draws. The
    // next frame begins at (0, 0) and rebuilds the plan.
    let Some(f) = cached else {
        return BG_COLOR;
    };

    let mut color = BG_COLOR;

    // 1. Status Bar (y < 60)
    if y < STATUS_BAR_HEIGHT + 12 {
        if let Some(c) = f.clock.pixel_on(x, y, BG_COLOR) {
            return c;
        }
        return color;
    }

    // 2. Status band (y in STATUS_BAND_TOP .. WORKSPACE_CARDS[0].y).
    // WHY a band test and not per-run boxes alone: `LabelRun::pixel` already
    // rejects a pixel outside its own measured line box, so this test exists to
    // keep the rest of the painter off the strip -- it returns the background
    // rather than falling through into the card loop, which would otherwise have
    // to reject every pixel here again.
    if y >= STATUS_BAND_TOP && y < WORKSPACE_CARDS[0].y {
        if let Some(c) = f.status_head.pixel_on(x, y, BG_COLOR) {
            return c;
        }
        if let Some(c) = f.status_body.pixel_on(x, y, BG_COLOR) {
            return c;
        }
        return color;
    }

    // 3. Cards. WHY the `None` arm continues rather than drawing: an absent plan
    // is a slot with no package id, and an outline with nothing in it would assert
    // a task the kernel did not list.
    for card in f.cards.iter() {
        let card = match card {
            Some(card) => card,
            None => continue,
        };
        if !rounded_contains(card.rect, x, y, CARD_RADIUS) {
            continue;
        }
        color = CARD_BG;
        if rounded_outline(card.rect, x, y, CARD_RADIUS) {
            return CARD_BORDER;
        }

        // The app's own icon: its pictogram when the package id maps to one, and
        // the registry's own badge word when it does not.
        if rounded_contains(card.icon_rect, x, y, CARD_ICON_RADIUS) {
            color = CARD_ICON_BG;
            if rounded_outline(card.icon_rect, x, y, CARD_ICON_RADIUS) {
                return CARD_ICON_BORDER;
            }
            if let Some(kind) = card.icon {
                let cx = card.icon_rect.center_x();
                let cy = card.icon_rect.center_y();
                if let Some(ic) = icons::rasterize_icon_pixel(kind, x, y, cx, cy, 0xFFFFFFFF) {
                    return ic;
                }
                return color;
            }
            if let Some(c) = card.badge.pixel_on(x, y, CARD_ICON_BG) {
                return c;
            }
            return color;
        }

        // Title & Subtitle
        if let Some(c) = card.title.pixel_on(x, y, CARD_BG) {
            return c;
        }
        if let Some(c) = card.subtitle.pixel_on(x, y, CARD_BG) {
            return c;
        }

        // Dismiss badge [X]
        if let Some(dismiss_r) = card.dismiss_rect {
            if rounded_contains(dismiss_r, x, y, BADGE_RADIUS) {
                color = 0xFF222734;
                if rounded_outline(dismiss_r, x, y, BADGE_RADIUS) {
                    return 0xFF374151;
                }
                let cx = dismiss_r.center_x() as i32;
                let cy = dismiss_r.center_y() as i32;
                let dx = (x as i32) - cx;
                let dy = (y as i32) - cy;
                // Crisp cross icon
                if dx.abs() <= 6 && dy.abs() <= 6 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                    return 0xFFE2E8F0;
                }
                return color;
            }
        }

        // Corner close control. WHY after the dismiss badge: the two never
        // overlap -- the badge's box starts 22px below this control's bottom edge
        // -- so the order only decides which one answers if a future card grows far
        // enough to overlap them.
        if let Some(close) = card.close.as_ref() {
            if rounded_contains(close.rect, x, y, CLOSE_RADIUS) {
                color = CLOSE_FILL;
                if rounded_outline(close.rect, x, y, CLOSE_RADIUS) {
                    return CLOSE_BORDER;
                }
                // WHY a live mark or a dash, and not a dimmed live mark: this
                // control stops a real kernel task, so when there is no task behind
                // the card it must say so rather than offer a press that terminates
                // nothing.
                if close.task != 0 {
                    return close_mark_pixel(close, x, y).unwrap_or(color);
                }
                return close_dash_pixel(close, x, y).unwrap_or(color);
            }
        }

        return color;
    }

    // 3b. Activity workspaces (contextual activities, design system section 4).
    // WHY paint-only: the whole block is below the deck and outside every band
    // the main.rs ActivitySpace handler owns, so nothing here answers a touch and
    // none of it may be styled as a live control. The heading and subheading sit
    // over the page background; each row draws its own card fill, then its ink.
    if let Some(c) = f.workspaces.heading.pixel_on(x, y, BG_COLOR) {
        return c;
    }
    if let Some(c) = f.workspaces.sub.pixel_on(x, y, BG_COLOR) {
        return c;
    }
    for row in f.workspaces.rows.iter() {
        if !rounded_contains(row.rect, x, y, ACTIVITY_RADIUS) {
            continue;
        }
        if rounded_outline(row.rect, x, y, ACTIVITY_RADIUS) {
            return WS_BORDER;
        }
        if let Some(c) = row.name.pixel_on(x, y, WS_CARD) {
            return c;
        }
        if let Some(c) = row.count.pixel_on(x, y, WS_CARD) {
            return c;
        }
        return WS_CARD;
    }

    // 4. Nav bar under the deck (covers y in 1220..1264, tap 360, 1240). WHY the
    // shell sends that band home rather than closing anything: see `NAV_HOME_BTN`.
    if f.nav_home_btn.contains(x, y) {
        if rounded_contains(f.nav_home_btn, x, y, 22) {
            color = 0xFF1E2532;
            if rounded_outline(f.nav_home_btn, x, y, 22) {
                return 0xFF334155;
            }
            if let Some(c) = f.nav_home_label.pixel(x, y) {
                return c;
            }
            return color;
        }
    }

    // 5. "+ New Activity" availability control and its reason lines, below the
    // home bar. WHY recessed and not raised: with no workspace store and no
    // `settings_set` authority in the launcher's capability table there is nothing
    // this control could really add, so it is drawn unavailable with that reason
    // rather than styled like a live action -- a dead control is worse than an
    // honest absence. It sits in no input band, so a tap does nothing by design.
    if rounded_contains(f.workspaces.new_rect, x, y, radius::CONTROL) {
        if rounded_outline(f.workspaces.new_rect, x, y, radius::CONTROL) {
            return WS_BORDER;
        }
        if let Some(c) = f.workspaces.new_label.pixel_on(x, y, WS_CARD_INERT) {
            return c;
        }
        return WS_CARD_INERT;
    }
    if let Some(c) = f.workspaces.note_a.pixel_on(x, y, BG_COLOR) {
        return c;
    }
    if let Some(c) = f.workspaces.reason.pixel_on(x, y, BG_COLOR) {
        return c;
    }

    // 6. Home Indicator Pill
    if rounded_contains(f.home_pill, x, y, 2) {
        return 0xFF64748B;
    }

    color
}

// -----------------------------------------------------------------------------
// Close control: hit test and the real termination behind it
// -----------------------------------------------------------------------------

/// The recent slot whose corner close control covers (`x`, `y`), if any.
///
/// WHY this lives here rather than in the shell's tap handler: that handler
/// carries its own copy of this screen's coordinates -- it hardcodes
/// `586..634 x 206..254` and two more bands for the dismiss badges -- so the
/// region a touch acted on and the pixels the touch landed on were two sets of
/// numbers held together by hand. This asks the screen that drew the control
/// where the control is, through the same [`close_control_for`] the painter used,
/// so a tap cannot point at a place the pixels are not.
///
/// WHY it answers only for a control that is actually drawn: card 3 has no recent
/// slot behind it, so a touch in that card's corner is not a close and this
/// returns `None`.
pub fn activity_close_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    let mut slot = 0usize;
    while slot < WORKSPACE_CARDS.len() {
        if let Some(control) = close_control_for(state, slot) {
            if control.rect.contains(x, y) {
                return Some(slot);
            }
        }
        slot += 1;
    }
    None
}

/// Closes recent slot `slot`: stops the task the kernel has for it and drops the
/// slot from the recent stack.
///
/// Returns the task id that was stopped, or `None` when the slot holds no running
/// task or the kernel refused the stop.
///
/// WHY `interos_std::app_stop` and not a local state edit: this screen is a
/// renderer and has no other channel to the kernel. `app_stop` is `sys_app_stop`,
/// which terminates the launcher's own child, reaps it and cancels its
/// notifications -- an edit to `LauncherState` alone would have hidden a task
/// that kept running, which is the worst possible answer to "close this app".
///
/// WHY a refusal leaves the slot alone: if the kernel would not stop the task it
/// is still running, and dropping it from the recent stack would be the launcher
/// claiming a process is gone while it is not.
pub fn activity_close_slot(state: &mut LauncherState, slot: usize) -> Option<u64> {
    let task = close_control_for(state, slot)?.task;
    if task == 0 {
        return None;
    }
    if interos_std::app_stop(task).is_err() {
        return None;
    }
    if state.active_child == Some(task) {
        state.active_child = None;
    }
    state.dismiss_recent(slot);
    Some(task)
}

/// [`activity_close_hit`] followed by [`activity_close_slot`]: closes whatever the
/// close control at (`x`, `y`) belongs to.
///
/// Returns the stopped task id, or `None` if there was no control there, the card
/// held no running task, or the kernel refused.
pub fn activity_close_at(state: &mut LauncherState, x: usize, y: usize) -> Option<u64> {
    let slot = activity_close_hit(state, x, y)?;
    activity_close_slot(state, slot)
}