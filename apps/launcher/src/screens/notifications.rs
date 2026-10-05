//! Notifications shade.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode.
//! Geometry and colour come from `launcher::theme`, `launcher::layout` and
//! `launcher::text`, never from literals local to this file.
//!
//! WHY the rows, the plan and the frame come from `super::system_space`: this
//! surface and the settings page are siblings reached from adjacent regions of the
//! same status bar, and a reader sees them back to back. A shade row is the same kind
//! of object as a settings row -- a label, a value column, a row plane inside a
//! rounded card -- so both surfaces call the same `CellPlan::row` and the same
//! `CardPlan`. Two row renderers would be two answers to "how tall is a row on this
//! shell" and to "what does an unavailable row look like", and the two surfaces would
//! drift the first time either was adjusted.
//!
//! WHY there are no Today and Yesterday headings: the reference design groups entries
//! under those two headings, and an entry can only be placed in either of them if it
//! carries a timestamp. The kernel's notification manager stores `timestamp_ticks`,
//! package, title and body for every record -- and its `NotificationQuery` syscall
//! returns `active.len()`, a number, and nothing else. `LauncherState` therefore
//! holds a count and not one entry, so there is nothing to group. Drawing the two
//! headings anyway would put two empty sections on a device that may have six
//! notifications waiting, which reads as "nothing arrived today" when the truth is
//! "nothing is published to me". So the shade reports what it was given, groups
//! nothing, and names what is missing. The grouping code is not absent, it is driven
//! by [`day_groups`], which is honest about returning zero.
//!
//! WHY the three groups are COUNT, ENTRIES and ACTIONS: the count is the one reading
//! the shade has, the two entry fields are what it is asked for and cannot supply,
//! and the action is the last thing a reader looks for. Separating them puts the one
//! real reading alone at the top instead of making it one row among refusals.
//!
//! WHY there is no Clear all control: clearing needs an id to cancel, the shell has
//! no ids, and the handler in `main.rs` resets its local counter and navigates away
//! without ever issuing `notification_cancel`. A button that empties a number on this
//! screen while the notifications stay on the device is a control that reports work
//! it did not do.

use super::system_space::{
    counted, header_h, round_contains, row_pad_x, CardPlan, CellPlan, Chip, Reading, Run, CARD_GAP, GROUP_ROW_H,
};
use crate::LauncherState;
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

/// WHY `Theme::LIGHT`: the reference design splits its palette by context, and the
/// notifications shade is one of the light surfaces -- a near-white page, white cards,
/// dark ink -- rather than the near-black the Power drawer uses. It is also the
/// settings page's sibling, and two adjacent surfaces with different palettes would
/// read as two different products.
const THEME: Theme = Theme::LIGHT;

/// Inset from the sheet's edge to the cards inside it.
///
/// WHY larger than a card's own text inset: these cards are inset *inside* a sheet,
/// so this is a card-to-sheet gap and not a card-edge-to-text gap, and it has to read
/// as one more level of nesting.
const SHEET_PAD: usize = space::XL;

/// Cards one shade can hold: the three groups below, plus the card that names the
/// fields a sheet could not fit.
const SHADE_CARDS: usize = 5;

/// Lines in the note printed under the groups.
const NOTE_LINES: usize = 3;

/// Gap between two lines of that note.
const NOTE_GAP: usize = space::SM;

/// Height of the note block, gaps included.
fn note_h() -> usize {
    NOTE_LINES * text::line_height(type_scale::CAPTION) + (NOTE_LINES - 1) * NOTE_GAP
}

/// -----------------------------------------------------------------------------
// What the shade can and cannot know
// -----------------------------------------------------------------------------

/// One field of the notification subsystem the shade would show if the subsystem
/// published it.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    /// The one reading that exists: how many records are active for this account.
    Active,
    /// Grouping entries under a day needs each entry's timestamp.
    Grouping,
    /// An unread badge needs a per-entry read flag and a total, not a count.
    Unread,
    /// Cancelling needs the notification ids the query does not return.
    Clear,
}

impl Field {
    fn label(self) -> &'static [u8] {
        match self {
            Field::Active => b"Notifications",
            Field::Grouping => b"Day grouping",
            Field::Unread => b"Unread count",
            Field::Clear => b"Clear all",
        }
    }

    /// What is missing, used verbatim as the row's value.
    ///
    /// WHY each reason names the specific missing part of the interface rather than
    /// saying "unsupported": a reader who has just seen a count of five can tell from
    /// this string exactly which field is absent, and can tell the difference between
    /// "the shade is broken" and "the pipe only carries a number".
    fn reason(self) -> &'static [u8] {
        match self {
            Field::Active => b"none active",
            Field::Grouping => b"no timestamps published",
            Field::Unread => b"no per-entry read flag",
            Field::Clear => b"no entry ids to cancel",
        }
    }
}

/// Whether anything reported this field.
///
/// WHY this is separate from [`value`]: the shade chooses between its empty state and
/// its list from this one bit, and a count of zero has to be distinguishable from a
/// count the query refused to return without building a value string to find out.
fn reported(field: Field, state: &LauncherState) -> bool {
    field == Field::Active && state.notification_count > 0
}

/// The value column entry for one field.
///
/// WHY the whole rule of this surface lives in this function: it must never print a
/// day, a time, a title or a count of unread where the platform has none of them to
/// give. Availability is decided next to the bytes that would be printed, which is
/// the only place the decision cannot be forgotten later.
fn value(field: Field, state: &LauncherState) -> Reading {
    match field {
        Field::Active if state.notification_count > 0 => {
            let (buf, len) = counted(state.notification_count, b" active");
            Reading::of(&buf[..len])
        }
        // WHY the grouping row is driven by `day_groups` rather than being a refusal
        // written out here: the count of groups is a real output of a real function,
        // and the row should print it the day the syscall grows an entry list. Today
        // that function returns zero, so the row is unavailable -- and unavailable
        // rather than "0 groups", because "0 groups" reads as "nothing arrived today",
        // which is a claim about the world. The truth is that the shell cannot see a
        // single entry, so there is no today to be empty.
        Field::Grouping => {
            let groups = day_groups(state);
            if groups == 0 {
                Reading::unavailable(field.reason())
            } else {
                let (buf, len) = counted(groups, b" shown");
                Reading::of(&buf[..len])
            }
        }
        Field::Active | Field::Unread | Field::Clear => Reading::unavailable(field.reason()),
    }
}

struct Group {
    header: &'static [u8],
    fields: &'static [Field],
}

static GROUPS: [Group; 3] = [
    Group { header: b"COUNT", fields: &[Field::Active] },
    Group { header: b"ENTRIES", fields: &[Field::Grouping, Field::Unread] },
    Group { header: b"ACTIONS", fields: &[Field::Clear] },
];

/// How many day groups the shade can honestly show.
///
/// WHY this is a function of the state and not two hardcoded headings: a group is a
/// set of entries that share a day, and an entry belongs to a day only if it carries
/// a time. `LauncherState` publishes `notification_count` and no per-entry record, so
/// there is nothing to group and the answer is zero. Two changes would make it
/// non-zero -- a syscall that returns the active records rather than their length, and
/// fields on `LauncherState` to hold them -- and neither is in this screen's gift, so
/// the shade says so instead of inventing entries to fill the headings.
fn day_groups(_state: &LauncherState) -> usize {
    0
}

/// The note printed under the groups when there is something to read.
///
/// WHY these three lines and not one: each names a different part of the interface
/// that is absent -- the records, the fields on them, and the ids needed to act on
/// them -- and collapsing them into "notifications unavailable" would leave a reader
/// who has just seen a count of five with no idea what the shade is failing to show.
///
/// WHY the lines are pre-broken rather than wrapped: `text::wrap` returns a
/// fixed `[usize; 3]` and would be free, but a note whose line breaks depend on the
/// measure of the text is a note whose height changes with its content, and the
/// sheet's height is derived from this block. Fixed lines keep the geometry stable.
const NOTE: [&[u8]; NOTE_LINES] = [
    b"The notification service publishes a count, not records.",
    b"Titles, bodies and timestamps are not exposed to this",
    b"shell, so entries cannot be grouped or dismissed.",
];

/// The empty state's three lines.
const EMPTY_LEDE: &[u8] = b"No notifications";
const EMPTY_SUB: &[u8] = b"Nothing is active for this account";
const EMPTY_NOTE: &[u8] = b"The service publishes a count only";

// -----------------------------------------------------------------------------
// The resolved frame
// -----------------------------------------------------------------------------

/// One resolved frame of the shade.
struct Frame {
    clock: Option<Run>,
    badge: Option<Chip>,
    title: Run,
    headline: Run,
    sheet: Rect,
    sheet_inner: Rect,
    cards: [CardPlan; SHADE_CARDS],
    count: usize,
    empty: [Run; 3],
    note: [Run; NOTE_LINES],
    indicator: Rect,
}

impl Frame {
    const EMPTY: Frame = Frame {
        clock: None,
        badge: None,
        title: Run::EMPTY,
        headline: Run::EMPTY,
        sheet: Rect::new(0, 0, 0, 0),
        sheet_inner: Rect::new(0, 0, 0, 0),
        cards: [CardPlan::EMPTY; SHADE_CARDS],
        count: 0,
        empty: [Run::EMPTY; 3],
        note: [Run::EMPTY; NOTE_LINES],
        indicator: Rect::new(0, 0, 0, 0),
    };
}

/// Height of one group's card inside the sheet.
fn group_h(group: &Group) -> usize {
    header_h() + group.fields.len() * GROUP_ROW_H
}

/// Places one group inside the sheet at `y`.
fn place_group(
    cards: &mut [CardPlan; SHADE_CARDS],
    count: &mut usize,
    sheet_left: usize,
    sheet_w: usize,
    group: &Group,
    y: usize,
    state: &LauncherState,
    theme: &Theme,
) -> bool {
    if *count >= SHADE_CARDS || group.fields.len() > 8 {
        return false;
    }
    let rect = Rect::new(sheet_left + SHEET_PAD, y, sheet_w - SHEET_PAD * 2, group_h(group));
    let mut card = CardPlan::with_header(rect, radius::CARD, group.header, theme);
    // WHY the cards here are `surface_sunken` and the settings page's are `surface`:
    // on the settings page the cards sit on the page, so they are raised one step.
    // Here they sit inside a sheet, so they are recessed one step. Same tokens, one
    // level deeper, which is what makes a sheet read as a sheet rather than as a
    // card floating on a card.
    card.set_fill(theme.surface_sunken);
    for (i, field) in group.fields.iter().enumerate() {
        let row_rect = Rect::new(rect.x, card.rows_top + i * GROUP_ROW_H, rect.w, GROUP_ROW_H);
        card.push(CellPlan::row(row_rect, field.label(), &value(*field, state), row_pad_x(), theme));
    }
    cards[*count] = card;
    *count += 1;
    true
}

/// Places the card that names the fields a sheet could not fit.
///
/// WHY it lists them rather than counting them: a shade whose every row is already a
/// refusal gains nothing from a row saying "3 more", while a card headed "NOT SHOWN"
/// that names each field tells the reader the shade is the short one.
fn place_overflow(
    cards: &mut [CardPlan; SHADE_CARDS],
    count: &mut usize,
    sheet_left: usize,
    sheet_w: usize,
    fields: &[Field],
    y: usize,
    state: &LauncherState,
    theme: &Theme,
) -> bool {
    if *count >= SHADE_CARDS || fields.is_empty() {
        return false;
    }
    let rect = Rect::new(
        sheet_left + SHEET_PAD,
        y,
        sheet_w - SHEET_PAD * 2,
        header_h() + fields.len() * GROUP_ROW_H,
    );
    let mut card = CardPlan::with_header(rect, radius::CARD, b"NOT SHOWN", theme);
    card.set_fill(theme.surface_sunken);
    card.set_header_ink(theme.warning);
    for (i, field) in fields.iter().enumerate() {
        let row_rect = Rect::new(rect.x, card.rows_top + i * GROUP_ROW_H, rect.w, GROUP_ROW_H);
        card.push(CellPlan::row(row_rect, field.label(), &value(*field, state), row_pad_x(), theme));
    }
    cards[*count] = card;
    *count += 1;
    true
}

/// Resolves one frame of the shade.
///
/// WHY the title block's geometry is identical to the settings page's: the two
/// surfaces are reached from adjacent regions of the same status bar, and a title
/// that sits at two different heights on two adjacent surfaces is the most obvious
/// way two screens fail to look like one system.
///
/// WHY the sheet is sized to its content rather than run to the bottom of the screen:
/// with no entries there is nothing to fill a full-height sheet, and a sheet with
/// three lines at the top and 700 px of blank surface below them reads as a rendering
/// fault. Sizing it to what it holds keeps the empty state a deliberate, compact
/// panel, and it keeps the shade's title at the same height as the settings page's
/// whatever the content is.
fn build(state: &LauncherState) -> Frame {
    let theme = THEME;
    let content = layout::content_area(CONTENT_PADDING);
    let cap_h = text::line_height(type_scale::CAPTION);

    let mut frame = Frame::EMPTY;

    // ---- status bar ------------------------------------------------------
    frame.clock = if state.clock_source_ok {
        Some(Run::whole(
            content.x,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            &state.clock_time,
            layout::ink_on(&theme, theme.overlay),
        ))
    } else {
        None
    };
    // WHY the badge repeats the count here as well as in the COUNT group: the status
    // bar is the one strip every surface in the shell shares, and a shade that is
    // reachable from it has to say what it is holding while the reader is still
    // looking at the bar.
    frame.badge = if state.notification_count > 0 {
        let (digits, dlen) = counted(state.notification_count, b"");
        let t = Run::right_edge(
            content.x + content.w - space::SM,
            (STATUS_BAR_HEIGHT - cap_h) / 2,
            type_scale::CAPTION,
            60,
            &digits[..dlen],
            theme.text_on_accent,
        );
        if t.is_empty() {
            None
        } else {
            Some(Chip::new(t, theme.accent))
        }
    } else {
        None
    };

    // ---- title block -----------------------------------------------------
    let title_h = text::line_height(type_scale::TITLE);
    let title_y = content.y;
    let head_y = title_y + title_h + space::XS;
    frame.title = Run::whole(content.x, title_y, type_scale::TITLE, b"Notifications", theme.text);
    {
        // WHY the headline is the count and nothing else: the notification service
        // publishes a count, so any adjective beside it would be invented. And the
        // empty case reads "none active" rather than "0 active" for the reason
        // `Field::Active::reason` gives.
        let headline = if reported(Field::Active, state) {
            let (buf, len) = counted(state.notification_count, b" active");
            Reading::of(&buf[..len])
        } else {
            Reading::unavailable(Field::Active.reason())
        };
        frame.headline = Run::whole(content.x, head_y, type_scale::CAPTION, headline.as_slice(), theme.text_muted);
    }

    // ---- the sheet --------------------------------------------------------
    let sheet_top = head_y + cap_h + space::XS;
    let floor = content.y + content.h;
    let sheet_left = content.x;
    let sheet_w = content.w;
    let inner_left = sheet_left + SHEET_PAD;
    let inner_w = sheet_w - SHEET_PAD * 2;
    let mut cards = [CardPlan::EMPTY; SHADE_CARDS];
    let mut count = 0usize;

    if !reported(Field::Active, state) {
        // ---- empty state -------------------------------------------------
        // WHY the sheet still fills the page even with nothing in it: a shade is an
        // overlay, and a small card floating at the top of an empty page reads as a
        // card that lost its content rather than as an overlay that has none. Filling
        // it and centring the block in it is what makes the emptiness the message.
        //
        // WHY the block's centre is above the sheet's centre: an empty state's optical
        // centre sits above its geometric centre, and a block pinned dead centre in a
        // sheet this tall reads as low.
        let body_h = text::line_height(type_scale::BODY);
        let block_h = body_h + space::SM + cap_h + space::MD + cap_h;
        let sheet_h = floor - sheet_top;
        frame.sheet = Rect::new(sheet_left, sheet_top, sheet_w, sheet_h);
        frame.sheet_inner = frame.sheet.inset(1);
        let mut ey = sheet_top + (sheet_h * 2 / 5) - block_h / 2;
        frame.empty[0] = Run::centred(inner_left, ey, type_scale::BODY, inner_w, EMPTY_LEDE, theme.text);
        ey += body_h + space::SM;
        frame.empty[1] = Run::centred(inner_left, ey, type_scale::CAPTION, inner_w, EMPTY_SUB, theme.text_muted);
        ey += cap_h + space::MD;
        frame.empty[2] = Run::centred(inner_left, ey, type_scale::CAPTION, inner_w, EMPTY_NOTE, theme.text_muted);
    } else {
        // ---- groups ------------------------------------------------------
        // WHY the note's height is reserved before the groups are placed: the
        // groups are what the reader came for, so they take the sheet first and the
        // note gets what is left, rather than the note claiming a fixed block and
        // pushing the last group off the bottom.
        let note_reserve = space::XXL + note_h();
        let mut gy = sheet_top + space::XL;
        let mut deferred: [Field; 4] = [Field::Active; 4];
        let mut deferred_len = 0usize;
        for group in GROUPS.iter() {
            let h = group_h(group);
            if count < SHADE_CARDS
                && gy + h + note_reserve <= floor
                && place_group(&mut cards, &mut count, sheet_left, sheet_w, group, gy, state, &theme)
            {
                gy += h + CARD_GAP;
            } else {
                for field in group.fields.iter() {
                    if deferred_len < deferred.len() {
                        deferred[deferred_len] = *field;
                        deferred_len += 1;
                    }
                }
            }
        }
        if deferred_len > 0
            && count < SHADE_CARDS
            && gy + header_h() + deferred_len * GROUP_ROW_H + note_reserve <= floor
            && place_overflow(
                &mut cards,
                &mut count,
                sheet_left,
                sheet_w,
                &deferred[..deferred_len],
                gy,
                state,
                &theme,
            )
        {
        }
        frame.count = count;
        frame.cards = cards;

        let pad = space::XXL;
        let sheet_h = floor - sheet_top;
        frame.sheet = Rect::new(sheet_left, sheet_top, sheet_w, sheet_h);
        frame.sheet_inner = frame.sheet.inset(1);

        // ---- note ---------------------------------------------------------
        let mut ny = sheet_top + sheet_h - pad - note_h();
        let mut k = 0usize;
        while k < NOTE_LINES {
            frame.note[k] = Run::whole(inner_left, ny, type_scale::CAPTION, NOTE[k], theme.text_muted);
            ny += cap_h + NOTE_GAP;
            k += 1;
        }
    }

    let indicator_w = space::XL * 4;
    frame.indicator = Rect::new(
        (SCREEN_WIDTH - indicator_w) / 2,
        SCREEN_HEIGHT - space::MD - space::XS,
        indicator_w,
        space::XS,
    );
    frame
}

/// This frame's plan.
///
/// WHY `(0, 0)` is the frame boundary: `render_view` walks the surface in raster
/// order from the top-left pixel, so `(0, 0)` is the first pixel of every frame and
/// the only pixel within a frame that repeats it. `LauncherState` is borrowed
/// immutably for the whole of a frame, so rebuilding there is once per frame and the
/// plan can never lag the state it describes.
static mut FRAME: Option<Frame> = None;

fn frame(x: usize, y: usize, state: &LauncherState) -> &'static Frame {
    let slot: *mut Option<Frame> = core::ptr::addr_of_mut!(FRAME);
    // SAFETY: the launcher is one bare-metal task and the paint loop is neither
    // preemptible nor reentrant, so this is the only reference to the slot at any
    // moment, and the reference returned cannot outlive the call that made it.
    let empty = unsafe { (*slot).is_none() };
    if empty || (x == 0 && y == 0) {
        // SAFETY: as above; this is the one mutation of the slot.
        unsafe { *slot = Some(build(state)) };
    }
    // SAFETY: the slot is `Some` here -- it was just filled when it was empty.
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}

pub fn get_notifications_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let theme = THEME;
    let f = frame(x, y, state);

    if y < STATUS_BAR_HEIGHT {
        if let Some(c) = f.clock.as_ref().and_then(|r| r.pixel(x, y)) {
            return c;
        }
        if let Some(chip) = &f.badge {
            if let Some(c) = chip.pixel(x, y) {
                return c;
            }
        }
        return theme.overlay;
    }
    if y == STATUS_BAR_HEIGHT {
        return theme.border;
    }
    if let Some(c) = f.title.pixel(x, y) {
        return c;
    }
    if let Some(c) = f.headline.pixel(x, y) {
        return c;
    }
    if !round_contains(&f.sheet, radius::SHEET, x, y) {
        return theme.bg;
    }
    if !round_contains(&f.sheet_inner, radius::SHEET - 1, x, y) {
        return theme.border;
    }

    let mut i = 0usize;
    while i < f.count {
        let card = &f.cards[i];
        if y < card.top() {
            break;
        }
        if let Some(c) = card.pixel(x, y, &theme) {
            return c;
        }
        i += 1;
    }
    let mut j = 0usize;
    while j < f.empty.len() {
        if let Some(c) = f.empty[j].pixel(x, y) {
            return c;
        }
        j += 1;
    }
    let mut k = 0usize;
    while k < f.note.len() {
        if let Some(c) = f.note[k].pixel(x, y) {
            return c;
        }
        k += 1;
    }
    theme.surface
}