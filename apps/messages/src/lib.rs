#![no_std]
//! InterOS Messages presentation, GSM-7 encoding, threat heuristic engine,
//! and circular bounded thread ledger store.

extern crate interos_std;

/// The app's view of the platform's design system.
pub mod theme;

pub mod gsm;
pub mod store;
pub mod privacy;
pub mod publish;
pub mod contacts;

/// The compose keyboard: the dock's geometry, the field it types into, and
/// what a key press does to that field.
pub mod keyboard;

pub const MAX_PHONE_LEN: usize = 32;
pub const MAX_PEER_HANDLE_LEN: usize = 32;
pub const MAX_SNIPPET_LEN: usize = 64;
pub const MAX_THREADS: usize = 8;
pub const MAX_MESSAGES_PER_THREAD: usize = 16;
pub const MAX_DRAFT_LEN: usize = 160;
pub const MAX_MESSAGE_BODY_LEN: usize = 160;

/// The label a record the app holds as incoming is drawn with.
///
/// WHY it says the record was written here: no transport is bound to this
/// app, so nothing can arrive. A record whose direction is incoming can
/// only be one this device wrote, and drawing it as "Received" would be a
/// claim about a channel the app has never had.
pub const INCOMING_LOCAL_LABEL: &[u8] = b"Written locally - never received";

/// The label the thread list draws for a thread whose previews are hidden.
///
/// WHY this is a constant and not a format: the whole point of the privacy
/// setting is that the snippet is not on the screen, so there is nothing to
/// format.
pub const HIDDEN_PREVIEW_LABEL: &[u8] = b"Message";

// -----------------------------------------------------------------------------
// The lanes the painter draws into
//
// WHY these live in the library and not beside the per-pixel branch that draws
// them: this app's binary is built with `test = false` (see
// `apps/messages/Cargo.toml`), so a fit gate can only be a `#[test]` inside this
// library, and a `#[test]` can only ask about a container it can reach. Every
// width below used to be a number written inside the branch that painted it --
// `624` for the empty state's query line, `CARD_X1 - CARD_X0 - 180` for a thread
// row's snippet, `rect.2 - 48` for a notice card, `490 - RECIPIENT_TEXT_X` for
// the recipient field -- with no constant naming what the number was or which box
// it came from. That is the defect class where a real clip passes the suite:
// raise a rung, the run grows, the remembered lane stays, and nothing answers.
//
// Every lane is derived forward from the box the painter actually draws into, and
// every gap of air is its own named constant, so moving the box moves the lane
// and the gate with it. The rungs come from the platform's ladder rather than
// being typed as multipliers, because a draw site that types its own multiplier
// can measure a run at one rung and ink it at another.
// -----------------------------------------------------------------------------

/// The geometry this app paints into, and the lane every fitted run answers.
pub mod layout {
    use crate::theme::{spacing, type_scale};

    // --- the boxes -----------------------------------------------------------

    /// The card every full-width row of this app is drawn on: the thread list's
    /// cards, the details and picker report rows, and the notice card.
    pub const LIST_CARD_X: usize = 24;
    pub const LIST_CARD_W: usize = 672;
    pub const LIST_CARD_RIGHT: usize = LIST_CARD_X + LIST_CARD_W;

    /// Air a run keeps from the card that carries it, on each side.
    pub const CARD_PAD_X: usize = spacing::GRID_24;

    /// The empty-state card: the same column and width as a row card, so a query
    /// line that fits one fits the other.
    pub const EMPTY_CARD_X: usize = LIST_CARD_X;
    pub const EMPTY_CARD_W: usize = LIST_CARD_W;

    // --- the thread row's columns -------------------------------------------

    /// The avatar disc's centre and radius, as the painter inks them.
    pub const AVATAR_CX: usize = LIST_CARD_X + 44;
    pub const AVATAR_R: usize = 24;

    /// The text column both of a thread row's runs start in.
    pub const ROW_TEXT_X: usize = LIST_CARD_X + 84;

    /// The unread pill's own box, and the delete mark's column and half-width.
    /// The pill is the trailing mark that binds a row's lanes: it shares its
    /// rows with both runs (`card_top + 34 .. card_top + 68` covers the peer
    /// name's box and the snippet's), and a lane that only clears the card's
    /// edge runs under it.
    pub const UNREAD_PILL_X: usize = LIST_CARD_RIGHT - 92;
    pub const UNREAD_PILL_RIGHT: usize = LIST_CARD_RIGHT - 56;
    pub const DELETE_MARK_X: usize = LIST_CARD_RIGHT - 38;
    pub const DELETE_MARK_HALF: usize = 6;

    /// Slack a run measured at one face keeps before the mark it sits beside.
    ///
    /// WHY it is named: the peer name and the header title were trimmed with a
    /// bare `12` written at the draw site. A slack that has no name cannot be
    /// re-measured when the mark moves, and it is the only thing standing between
    /// a fitted run and the pill's left edge.
    pub const MARK_TRIM_SLACK: usize = 12;

    // --- the search / recipient / compose fields ----------------------------

    /// The search field's box, its text column, and the clear mark that ends it.
    pub const SEARCH_FIELD_X: usize = 24;
    pub const SEARCH_FIELD_W: usize = 672;
    pub const SEARCH_TEXT_X: usize = 82;
    pub const SEARCH_CLEAR_CX: usize = 672;
    pub const SEARCH_CLEAR_HALF_W: usize = 16;
    pub const SEARCH_CLEAR_X0: usize = SEARCH_CLEAR_CX - SEARCH_CLEAR_HALF_W;

    /// The recipient field's box and its text column.
    pub const RECIPIENT_FIELD_X: usize = 24;
    pub const RECIPIENT_FIELD_W: usize = 586;
    pub const RECIPIENT_FIELD_RIGHT: usize = RECIPIENT_FIELD_X + RECIPIENT_FIELD_W;
    pub const RECIPIENT_TEXT_X: usize = 76;

    /// The clear mark both editors carry, as the painter clips it.
    pub const FIELD_CLEAR_CX: usize = 590;
    pub const FIELD_CLEAR_HALF_W: usize = 16;
    pub const FIELD_CLEAR_X0: usize = FIELD_CLEAR_CX - FIELD_CLEAR_HALF_W;
    pub const FIELD_CLEAR_X1: usize = FIELD_CLEAR_CX + FIELD_CLEAR_HALF_W;

    /// The air a field's text keeps from the mark it is typed beside.
    pub const FIELD_MARK_PAD: usize = spacing::GRID_8;

    /// The compose field's text column, the counter's right edge, and the room
    /// the counter is given for its widest line (`used/limit`).
    pub const COMPOSE_FIELD_X: usize = 24;
    pub const COMPOSE_FIELD_W: usize = 586;
    pub const COMPOSE_TEXT_X: usize = 52;
    pub const COMPOSE_COUNT_RIGHT: usize = 572;
    pub const COMPOSE_COUNT_RESERVE: usize = 56;

    /// The buffers the three editors are bounded by. They live here because the
    /// counter's denominator and the widest run an editor can hold are the same
    /// numbers, and a fit gate has to measure the worst case rather than a
    /// typical one.
    pub const COMPOSE_CAP: usize = 160;
    pub const SEARCH_CAP: usize = 48;
    pub const RECIP_CAP: usize = 32;

    /// The recipient picker's control: the column it owns at the right end of
    /// the recipient band, the width it is painted with, and the air its centred
    /// label keeps from its own border.
    ///
    /// WHY the box is here: the label inside it is centred with no trim, so
    /// "does `PICK` still read inside its control" is a lane question, and the
    /// lane answers to the control's width.
    pub const PICK_CONTROL_X: usize = LIST_CARD_RIGHT - 80;
    pub const PICK_CONTROL_W: usize = 80;
    pub const CONTROL_LABEL_PAD: usize = spacing::GRID_8;

    /// The two labels that control can carry. Nothing trims either of them -- the
    /// painter centres them in the control -- so the only honest check is that the
    /// widest one still reads inside the box it is inked on, which is what the
    /// library's fit gate measures.
    pub const PICK_LABEL: &[u8] = b"PICK";
    pub const INFO_LABEL: &[u8] = b"INFO";

    /// The `To:` prefix's column, in front of the recipient editor's text column.
    pub const RECIPIENT_PREFIX_X: usize = 40;

    /// The prefix itself. It is drawn with no trim, so the lane beside it is the
    /// only thing that can say whether it still stops short of the typed address.
    pub const RECIPIENT_PREFIX_LABEL: &[u8] = b"To:";

    /// The unread pill's count column: the inset from the pill's left edge the
    /// painter inks the digits at.
    pub const UNREAD_COUNT_INSET: usize = 12;

    /// The conversation view's status banner columns: the run it starts with when
    /// the threat glyph leads and the column it starts in when nothing does.
    pub const BANNER_GLYPH_TEXT_X: usize = 76;
    pub const BANNER_PLAIN_TEXT_X: usize = 40;

    /// The header's title column.
    pub const HEADER_TITLE_X: usize = 116;

    /// The air a bubble keeps between its own edge and the run it carries.
    pub const BUBBLE_PAD_X: usize = 16;

    /// The narrowest a chat bubble may be drawn, so a one-word message still
    /// reads as a bubble, and the widest, so a long message stops short of the
    /// screen edge.
    ///
    /// WHY they live here: the bubble hugs its content, so the lane a bubble's
    /// body is trimmed to is only as wide as the clamp lets the bubble grow -- and
    /// a status line that cannot be drawn inside the widest bubble is a status
    /// line the screen never shows whole.
    pub const BUBBLE_MIN_W: usize = 168;
    pub const BUBBLE_MAX_W: usize = 488;

    // --- the rungs, named ---------------------------------------------------

    pub const PEER_NAME_RUNG: usize = type_scale::BODY;
    pub const SNIPPET_RUNG: usize = type_scale::CAPTION;
    pub const ROW_CAPTION_RUNG: usize = type_scale::CAPTION;
    pub const ROW_BODY_RUNG: usize = type_scale::BODY;
    pub const NOTICE_RUNG: usize = type_scale::CAPTION;
    pub const EMPTY_QUERY_RUNG: usize = type_scale::CAPTION;
    /// The empty state's own headline rungs: the search-refusal line reads at the
    /// caption rung, the zero-thread card's title at the body rung.
    pub const EMPTY_HEADLINE_RUNG: usize = type_scale::CAPTION;
    pub const EMPTY_TITLE_RUNG: usize = type_scale::BODY;
    pub const EMPTY_SUBLINE_RUNG: usize = type_scale::CAPTION;
    pub const BUBBLE_BODY_RUNG: usize = type_scale::BODY;
    pub const BUBBLE_STATUS_RUNG: usize = type_scale::CAPTION;
    pub const FIELD_TEXT_RUNG: usize = type_scale::BODY;

    /// The remaining rungs this app inks at, named rather than typed at the draw
    /// site.
    ///
    /// WHY they belong to the ladder and not to the branch: a site that writes its
    /// own multiplier can measure a run at one rung and ink it at another, and
    /// nothing in the suite answers -- which is exactly how a remembered `1` ended
    /// up beside a `measure` call that meant the caption rung.
    pub const CONTROL_LABEL_RUNG: usize = type_scale::CAPTION;
    pub const FIELD_PREFIX_RUNG: usize = type_scale::CAPTION;
    pub const BANNER_RUNG: usize = type_scale::CAPTION;
    pub const CONVO_EMPTY_RUNG: usize = type_scale::CAPTION;
    pub const COUNTER_RUNG: usize = type_scale::CAPTION;
    pub const UNREAD_COUNT_RUNG: usize = type_scale::CAPTION;

    // --- the lanes ----------------------------------------------------------

    /// The lane a card's full-width row gives its run: the card's own width less
    /// the air on either side. A `const fn` because the painter builds the rect it
    /// draws into at the call site, and the fit has to answer that rect and not a
    /// remembered copy of it.
    pub const fn card_row_lane(w: usize) -> usize {
        if w > 2 * CARD_PAD_X {
            w - 2 * CARD_PAD_X
        } else {
            0
        }
    }

    /// A report row's lane: the card the painter builds at
    /// `(CARD_X0, top, CARD_X1 - CARD_X0, ROW_CARD_H)`.
    pub const REPORT_ROW_LANE: usize = card_row_lane(LIST_CARD_W);

    /// The empty state's query line, centred inside the empty card.
    pub const EMPTY_CARD_LANE: usize = card_row_lane(EMPTY_CARD_W);

    /// The notice card's lane: the card's width less the air the run is painted
    /// in by, which is where the painter puts it (`rect.0 + CARD_PAD_X`).
    pub const NOTICE_LANE: usize = card_row_lane(LIST_CARD_W);

    /// The lane a thread row's peer name owns: from the text column to the
    /// unread pill's left edge, less the slack that keeps it off the pill.
    pub const PEER_NAME_LANE: usize = UNREAD_PILL_X - ROW_TEXT_X - MARK_TRIM_SLACK;

    /// The lane a thread row's snippet owns.
    ///
    /// WHY it is spelled out instead of aliasing the name lane's arithmetic: the
    /// two runs stand on the same two edges of the same card, so they answer the
    /// same lane -- but the painter used to trim the snippet to
    /// `CARD_X1 - CARD_X0 - 180`, a remembered number that stopped four columns
    /// short of the pill and eight past the peer name's own bound. A snippet is
    /// now cut where the name is cut.
    pub const SNIPPET_LANE: usize = UNREAD_PILL_X - ROW_TEXT_X - MARK_TRIM_SLACK;

    /// The lane the typed query owns inside the search field: it stops at the
    /// clear mark's left arm, less the field's mark pad.
    pub const SEARCH_TEXT_LANE: usize = SEARCH_CLEAR_X0 - FIELD_MARK_PAD - SEARCH_TEXT_X;

    /// The lane the recipient editor owns: it stops at the clear mark's left arm.
    ///
    /// WHY the remembered `490` is not kept as a literal: it is this same
    /// distance, measured by hand off the mark; the day the mark moves, the
    /// remembered number is what lets the typed address run under it.
    pub const RECIPIENT_TEXT_LANE: usize = FIELD_CLEAR_X0 - FIELD_MARK_PAD - RECIPIENT_TEXT_X;

    /// The lane the compose editor owns: it stops where the character counter's
    /// reserved room begins.
    pub const COMPOSE_TEXT_LANE: usize =
        COMPOSE_COUNT_RIGHT - COMPOSE_COUNT_RESERVE - COMPOSE_TEXT_X;

    /// The header title's lane: the pill's measured left edge, less the title's
    /// own column and the slack that keeps the run off the pill.
    pub const fn header_title_lane(pill_left: usize) -> usize {
        pill_left.saturating_sub(HEADER_TITLE_X + MARK_TRIM_SLACK)
    }

    /// A bubble's text lane: the bubble's own width less the air on either side.
    /// The bubble hugs its content, so the lane is only known once the hug is.
    pub const fn bubble_text_lane(bubble_w: usize) -> usize {
        if bubble_w > 2 * BUBBLE_PAD_X {
            bubble_w - 2 * BUBBLE_PAD_X
        } else {
            0
        }
    }

    /// The room a bubble's text takes on top of its own width when the bubble
    /// hugs its content: the same air the text lane removes.
    pub const BUBBLE_HUG_PAD: usize = 2 * BUBBLE_PAD_X;

    /// A control's centred label lane: the control's own width less the air it
    /// keeps from its border on either side. Nothing trims this run, so the gate
    /// is the label fitting at all.
    pub const CONTROL_LABEL_LANE: usize = PICK_CONTROL_W - 2 * CONTROL_LABEL_PAD;

    /// The lane the `To:` prefix owns: from its own column to the editor's text
    /// column, which is where the typed address starts.
    pub const RECIPIENT_PREFIX_LANE: usize = RECIPIENT_TEXT_X - RECIPIENT_PREFIX_X;

    /// The lane the unread count owns inside its pill: from the digit column to
    /// the pill's right edge.
    pub const UNREAD_COUNT_LANE: usize = UNREAD_PILL_RIGHT - (UNREAD_PILL_X + UNREAD_COUNT_INSET);

    /// The conversation view's status banner lanes: from the column the run starts
    /// in -- after the threat glyph, or at the field's own padding when there is no
    /// glyph -- to the air inside the band's right edge.
    pub const BANNER_GLYPH_LANE: usize = LIST_CARD_RIGHT - CARD_PAD_X - BANNER_GLYPH_TEXT_X;
    pub const BANNER_PLAIN_LANE: usize = LIST_CARD_RIGHT - CARD_PAD_X - BANNER_PLAIN_TEXT_X;

    /// The lane a run centred on the bare panel owns: the screen less the side
    /// margin every card of this app stands inside, so a centred line never reads
    /// wider than the cards it sits between.
    pub const PANEL_CENTER_LANE: usize =
        interos_std::design::viewport::LOGICAL_WIDTH - 2 * LIST_CARD_X;

    // --- the runs that are never trimmed -----------------------------------

    /// The fixed strings this app paints with no `ellipsize` in front of them,
    /// gathered next to the lanes that hold them.
    ///
    /// WHY they are here: an unellipsized run has no fit step to check, so the
    /// only thing that can say it still fits is a gate that measures the literal
    /// itself at the rung the painter inks it at. Measuring it inside the branch
    /// that draws it would be the same number twice.
    pub const EMPTY_FILTER_LINE: &[u8] = b"No chats match your search";
    /// What the empty state falls back to when the lane cannot even hold the
    /// ellipsis `ellipsize` appends -- the report of what happened, not a silence.
    pub const EMPTY_FILTER_FALLBACK: &[u8] = b"(empty query)";
    pub const EMPTY_THREADS_TITLE: &[u8] = b"No Conversations Yet";
    pub const EMPTY_TRANSPORT_SUBLINE: &[u8] = b"Messaging unavailable: no transport";
    pub const EMPTY_STORE_REFUSED_SUBLINE: &[u8] =
        b"Stored conversations could not be read";
    pub const EMPTY_STORE_INVALID_SUBLINE: &[u8] =
        b"Stored conversations failed validation";
    pub const EMPTY_STORE_FULL_SUBLINE: &[u8] =
        b"Conversation store is full; older threads were not kept";

    /// The empty conversation's two centred lines, drawn on the bare panel.
    pub const CONVO_EMPTY_LINE1: &[u8] = b"No messages in this chat yet";
    pub const CONVO_EMPTY_LINE2: &[u8] = b"Type a draft below to compose";

    /// The notice card's own copy when there is nothing to notice, and the
    /// picker card's when no address has been chosen.
    pub const NOTICE_EMPTY_TEXT: &[u8] = b"Nothing has been published or shared yet.";
    pub const PICKER_NO_RECIPIENT_TEXT: &[u8] = b"No recipient chosen";

    /// The widest unread count the model can hand the pill: `unread_count` is a
    /// `u8` the list only ever saturates, so three digits is the worst line the
    /// pill can be asked to hold.
    pub const UNREAD_COUNT_WIDEST: &[u8] = b"255";

    /// The run a container hands its painter: `label` whole when the container
    /// holds it, otherwise the same label cut to `lane` with an ellipsis on it.
    ///
    /// WHY it lives in the model: every call site is a per-pixel branch this
    /// app's binary cannot test, and the claim a fit gate is worth making -- a run
    /// this app draws is either whole or visibly cut, never silently missing --
    /// has to be checked against the same function the painter calls. A lane
    /// narrower than the ellipsis itself answers `font::ellipsize` with nothing,
    /// which is the one case where a word would vanish rather than be trimmed; the
    /// gates below refuse that for every lane in this module.
    pub fn fitted_run<'a>(label: &'a [u8], rung: usize, lane: usize, buf: &'a mut [u8]) -> &'a [u8] {
        let len = interos_std::design::font::ellipsize(label, rung, lane, buf);
        if len == 0 {
            return label;
        }
        &buf[..len]
    }

    /// Truth gates: every lane is a lane -- it owns room of its own, it is not
    /// wider than the ellipsis that trims it, and it never reaches past the box it
    /// is drawn in. These are the checks that fire if a container moves and its
    /// lane is left where the remembered number had it.
    const _: () = assert!(LIST_CARD_RIGHT <= interos_std::design::viewport::LOGICAL_WIDTH);
    const _: () = assert!(CARD_PAD_X > 0);
    const _: () = assert!(AVATAR_CX + AVATAR_R < ROW_TEXT_X);
    const _: () = assert!(ROW_TEXT_X < UNREAD_PILL_X);
    const _: () = assert!(UNREAD_PILL_X < UNREAD_PILL_RIGHT);
    const _: () = assert!(UNREAD_PILL_RIGHT < DELETE_MARK_X);
    const _: () = assert!(DELETE_MARK_X + DELETE_MARK_HALF <= LIST_CARD_RIGHT);
    const _: () = assert!(MARK_TRIM_SLACK >= 1);
    const _: () = assert!(PEER_NAME_LANE > 0);
    const _: () = assert!(
        ROW_TEXT_X + PEER_NAME_LANE <= UNREAD_PILL_X,
        "a thread row's peer name must stop before the unread pill it is drawn beside"
    );
    const _: () = assert!(SNIPPET_LANE > 0);
    const _: () = assert!(
        ROW_TEXT_X + SNIPPET_LANE <= UNREAD_PILL_X,
        "a thread row's snippet must stop before the unread pill it is drawn beside"
    );
    const _: () = assert!(SNIPPET_LANE == PEER_NAME_LANE);
    const _: () = assert!(REPORT_ROW_LANE > 0);
    const _: () = assert!(
        LIST_CARD_X + CARD_PAD_X + REPORT_ROW_LANE <= LIST_CARD_RIGHT,
        "a report row's lane must end inside the card the row is drawn on"
    );
    const _: () = assert!(EMPTY_CARD_LANE > 0);
    const _: () = assert!(
        EMPTY_CARD_X + CARD_PAD_X + EMPTY_CARD_LANE <= EMPTY_CARD_X + EMPTY_CARD_W,
        "the empty state's query line must end inside the card that holds it"
    );
    const _: () = assert!(NOTICE_LANE > 0);
    const _: () = assert!(
        LIST_CARD_X + CARD_PAD_X + NOTICE_LANE <= LIST_CARD_RIGHT,
        "a notice card's line must end inside the card it is painted on"
    );
    const _: () = assert!(SEARCH_TEXT_LANE > 0);
    const _: () = assert!(
        SEARCH_TEXT_X + SEARCH_TEXT_LANE <= SEARCH_CLEAR_X0,
        "the typed query must stop before the clear mark it is typed beside"
    );
    const _: () = assert!(SEARCH_FIELD_X + SEARCH_FIELD_W <= LIST_CARD_RIGHT);
    const _: () = assert!(RECIPIENT_TEXT_LANE > 0);
    const _: () = assert!(
        RECIPIENT_TEXT_X + RECIPIENT_TEXT_LANE <= FIELD_CLEAR_X0,
        "the recipient editor must stop before the clear mark inside its field"
    );
    const _: () = assert!(FIELD_CLEAR_X1 <= RECIPIENT_FIELD_RIGHT);
    const _: () = assert!(COMPOSE_TEXT_LANE > 0);
    const _: () = assert!(
        COMPOSE_TEXT_X + COMPOSE_TEXT_LANE + COMPOSE_COUNT_RESERVE <= COMPOSE_COUNT_RIGHT,
        "the composer must stop where the character counter's reserved room begins"
    );
    const _: () = assert!(COMPOSE_COUNT_RIGHT < FIELD_CLEAR_X0);
    const _: () = assert!(header_title_lane(LIST_CARD_RIGHT) > 0);
    const _: () = assert!(bubble_text_lane(2 * BUBBLE_PAD_X) == 0);
    const _: () = assert!(bubble_text_lane(BUBBLE_MIN_W) > 3);
    const _: () = assert!(bubble_text_lane(BUBBLE_MAX_W) > 3);
    const _: () = assert!(BUBBLE_HUG_PAD == 2 * BUBBLE_PAD_X);
    /// Every lane has to be wider than the ellipsis `ellipsize` appends, or a
    /// trimmed run comes back as nothing and the word disappears instead of
    /// showing itself cut.
    const _: () = assert!(card_row_lane(LIST_CARD_W) > 3);
    const _: () = assert!(PEER_NAME_LANE > 3);

    // The lanes that bind a run to a mark it is drawn beside, rather than to a
    // card's edge.
    const _: () = assert!(RECIPIENT_FIELD_RIGHT <= PICK_CONTROL_X);
    const _: () = assert!(PICK_CONTROL_X + PICK_CONTROL_W <= LIST_CARD_RIGHT);
    const _: () = assert!(CONTROL_LABEL_LANE > 0);
    const _: () = assert!(
        PICK_CONTROL_X + (PICK_CONTROL_W - CONTROL_LABEL_LANE) / 2 + CONTROL_LABEL_LANE
            <= PICK_CONTROL_X + PICK_CONTROL_W,
        "a control's centred label must end inside the control it is inked on"
    );
    const _: () = assert!(RECIPIENT_PREFIX_LANE > 0);
    const _: () = assert!(
        RECIPIENT_PREFIX_X + RECIPIENT_PREFIX_LANE <= RECIPIENT_TEXT_X,
        "the `To:` prefix must stop where the typed address it sits beside begins"
    );
    const _: () = assert!(UNREAD_COUNT_LANE > 0);
    const _: () = assert!(
        UNREAD_PILL_X + UNREAD_COUNT_INSET + UNREAD_COUNT_LANE <= UNREAD_PILL_RIGHT,
        "the unread count must end inside the pill it is inked on"
    );
    const _: () = assert!(BANNER_GLYPH_LANE > 0);
    const _: () = assert!(BANNER_PLAIN_LANE > 0);
    const _: () = assert!(
        BANNER_GLYPH_TEXT_X + BANNER_GLYPH_LANE <= LIST_CARD_RIGHT - CARD_PAD_X,
        "the status banner's line must end inside the band it is painted on"
    );
    const _: () = assert!(PANEL_CENTER_LANE > 0);
    const _: () = assert!(
        LIST_CARD_X + PANEL_CENTER_LANE
            <= interos_std::design::viewport::LOGICAL_WIDTH - LIST_CARD_X,
        "a line centred on the panel must keep the side margin every card stands inside"
    );
    const _: () = assert!(COMPOSE_COUNT_RESERVE > 0);
}

/// Case-insensitive substring match.
pub fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            if haystack[i + j].to_ascii_lowercase() != needle[j].to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Whether a thread matches a search query.
pub fn thread_matches(tr: &ThreadRecord, query: &[u8]) -> bool {
    contains_ci(tr.peer(), query)
}

/// The index of the `n`th thread that matches `query`.
///
/// WHY the list is addressed by the matching index and not by the ledger's
/// own: a filtered list draws only the rows that match, so the row under the
/// finger is the `n`th match and not the `n`th thread. The two are the same
/// only when nothing is filtered.
pub fn filtered_thread_index(
    threads: &[ThreadRecord],
    thread_count: usize,
    query: &[u8],
    n: usize,
) -> Option<usize> {
    let mut seen = 0usize;
    for i in 0..thread_count.min(threads.len()) {
        if thread_matches(&threads[i], query) {
            if seen == n {
                return Some(i);
            }
            seen += 1;
        }
    }
    None
}

/// Message transmission direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    Incoming,
    Outgoing,
}

/// Status of a messaging conversation thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    EncryptedP2P = 0,
    CellularSms = 1,
    PendingHandshake = 2,
}

/// Whether a recipient is something a telephone could be dialled with.
///
/// WHY this exists: the recipient field accepts a name or a handle as well
/// as a number. Handing a name to a modem is not a transmission, it is a
/// mistake, and the message has to stay local instead of being reported as
/// a transmission that never happened.
pub fn is_dialable(recipient: &[u8]) -> bool {
    if recipient.is_empty() {
        return false;
    }
    recipient.iter().all(|&b| {
        b.is_ascii_digit() || matches!(b, b'+' | b' ' | b'-' | b'(' | b')' | b'.')
    })
}

/// The send control's arrow, in pixels relative to the control's centre.
///
/// WHY the barbs are measured back from the tip: they used to widen as `dx`
/// grew, which put the vertex on the left and drew a back arrow on the control
/// that sends. Both composer layouts ask this one predicate so the direction
/// has one owner and the test below can ask which way it points.
pub fn send_arrow_ink(dx: isize, dy: isize) -> bool {
    let a = dy.abs();
    (a <= 10 && dx >= 4 - a && dx <= 8 - a) || (a <= 2 && dx >= -10 && dx <= 8)
}

/// Truthful delivery status for SMS and P2P messaging.
///
/// WHY `LocalOnly` exists: a thread may be addressed to a name or a handle
/// rather than to a dialable number, and in that case no transport is
/// applicable and none is asked. That is a different fact from a transport
/// that was asked and refused, and the two are drawn differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    /// No transport has confirmed this message.
    Pending,
    /// The transport accepted the message.
    Sent,
    /// The transport refused the message.
    Failed,
    /// The transport refused the message for want of a bound transport.
    RefusedNoTransport,
    /// The recipient is a local handle, so the message stayed on the device.
    LocalOnly,
}

impl DeliveryStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Not sent",
            Self::Sent => "Sent",
            Self::Failed => "Failed",
            Self::RefusedNoTransport => "Refused - No Transport",
            Self::LocalOnly => "Local only - not sent",
        }
    }
}

/// Bounded individual message record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageRecord {
    pub id: u32,
    pub thread_id: u32,
    pub timestamp: u64,
    pub direction: MessageDirection,
    pub status: DeliveryStatus,
    pub body: [u8; MAX_MESSAGE_BODY_LEN],
    pub body_len: usize,
}

impl MessageRecord {
    pub const EMPTY: Self = Self {
        id: 0,
        thread_id: 0,
        timestamp: 0,
        direction: MessageDirection::Incoming,
        status: DeliveryStatus::Pending,
        body: [0; MAX_MESSAGE_BODY_LEN],
        body_len: 0,
    };

    pub fn text(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

/// Bounded conversation thread record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadRecord {
    pub id: u32,
    pub peer_handle: [u8; MAX_PEER_HANDLE_LEN],
    pub handle_len: usize,
    pub last_timestamp: u64,
    pub unread_count: u8,
    pub message_count: u8,
}

impl ThreadRecord {
    pub const EMPTY: Self = Self {
        id: 0,
        peer_handle: [0; MAX_PEER_HANDLE_LEN],
        handle_len: 0,
        last_timestamp: 0,
        unread_count: 0,
        message_count: 0,
    };

    pub fn peer(&self) -> &[u8] {
        &self.peer_handle[..self.handle_len]
    }
}

/// Circular message ring buffer for a single thread, holding up to 16 messages.
#[derive(Debug, Clone, Copy)]
pub struct ThreadMessageRing {
    pub messages: [MessageRecord; MAX_MESSAGES_PER_THREAD],
    pub head: usize,
    pub count: usize,
}

impl ThreadMessageRing {
    pub const EMPTY: Self = Self {
        messages: [MessageRecord::EMPTY; MAX_MESSAGES_PER_THREAD],
        head: 0,
        count: 0,
    };

    /// Pushes a message into the circular ring. When full, overwrites the oldest message.
    pub fn push(&mut self, record: MessageRecord) {
        self.messages[self.head] = record;
        self.head = (self.head + 1) % MAX_MESSAGES_PER_THREAD;
        if self.count < MAX_MESSAGES_PER_THREAD {
            self.count += 1;
        }
    }

    /// Retrieves messages in chronological order (oldest to newest).
    /// Returns the number of messages copied into `out`.
    pub fn get_messages(&self, out: &mut [MessageRecord]) -> usize {
        let n = self.count.min(out.len());
        let oldest = if self.count < MAX_MESSAGES_PER_THREAD {
            0
        } else {
            self.head
        };
        for i in 0..n {
            let idx = (oldest + i) % MAX_MESSAGES_PER_THREAD;
            out[i] = self.messages[idx];
        }
        n
    }

    /// Returns the most recent message posted to this ring, if any.
    pub fn latest(&self) -> Option<&MessageRecord> {
        if self.count == 0 {
            None
        } else {
            let idx = (self.head + MAX_MESSAGES_PER_THREAD - 1) % MAX_MESSAGES_PER_THREAD;
            Some(&self.messages[idx])
        }
    }
}

/// Zero-heap SMS message composition draft buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageDraft {
    pub recipient: [u8; MAX_PHONE_LEN],
    pub recipient_len: usize,
    pub text: [u8; MAX_DRAFT_LEN],
    pub len: usize,
}

impl MessageDraft {
    pub const fn new() -> Self {
        Self {
            recipient: [0; MAX_PHONE_LEN],
            recipient_len: 0,
            text: [0; MAX_DRAFT_LEN],
            len: 0,
        }
    }

    pub fn set_recipient(&mut self, rec: &[u8]) {
        let n = rec.len().min(MAX_PHONE_LEN);
        self.recipient = [0; MAX_PHONE_LEN];
        self.recipient[..n].copy_from_slice(&rec[..n]);
        self.recipient_len = n;
    }

    pub fn push_char(&mut self, c: u8) -> bool {
        if self.len < MAX_DRAFT_LEN {
            self.text[self.len] = c;
            self.len += 1;
            true
        } else {
            false
        }
    }

    pub fn pop_char(&mut self) -> Option<u8> {
        if self.len > 0 {
            self.len -= 1;
            Some(self.text[self.len])
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub const fn remaining_chars(&self) -> usize {
        MAX_DRAFT_LEN - self.len
    }
}

/// Production ThreadLedger managing up to 8 conversation threads,
/// each backed by a 16-message circular buffer.
#[derive(Debug, Clone, Copy)]
pub struct ThreadLedger {
    pub threads: [ThreadRecord; MAX_THREADS],
    pub rings: [ThreadMessageRing; MAX_THREADS],
    pub thread_count: usize,
    pub next_thread_id: u32,
    pub next_msg_id: u32,
    pub virtual_clock: u64,
}

impl ThreadLedger {
    pub const EMPTY: Self = Self {
        threads: [ThreadRecord::EMPTY; MAX_THREADS],
        rings: [ThreadMessageRing::EMPTY; MAX_THREADS],
        thread_count: 0,
        next_thread_id: 1,
        next_msg_id: 1,
        virtual_clock: 100,
    };

    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Finds an existing thread by peer handle or creates a new thread record.
    /// Returns the thread ID on success, or None if the ledger is full (8 threads max).
    pub fn create_or_get_thread(&mut self, peer: &[u8]) -> Option<u32> {
        let plen = peer.len().min(MAX_PEER_HANDLE_LEN);
        for i in 0..self.thread_count {
            if self.threads[i].handle_len == plen && &self.threads[i].peer_handle[..plen] == &peer[..plen] {
                return Some(self.threads[i].id);
            }
        }
        if self.thread_count < MAX_THREADS {
            let idx = self.thread_count;
            let tid = self.next_thread_id;
            self.next_thread_id = self.next_thread_id.wrapping_add(1);
            if self.next_thread_id == 0 {
                self.next_thread_id = 1;
            }

            let mut tr = ThreadRecord::EMPTY;
            tr.id = tid;
            tr.peer_handle[..plen].copy_from_slice(&peer[..plen]);
            tr.handle_len = plen;
            tr.last_timestamp = self.virtual_clock;
            tr.unread_count = 0;
            tr.message_count = 0;

            self.threads[idx] = tr;
            self.rings[idx] = ThreadMessageRing::EMPTY;
            self.thread_count += 1;
            Some(tid)
        } else {
            None
        }
    }

    /// Posts a message to a thread with default delivery status (RefusedNoTransport for outgoing, Sent for incoming).
    pub fn post_message(&mut self, thread_id: u32, dir: MessageDirection, body: &[u8]) -> Option<u32> {
        let status = match dir {
            MessageDirection::Incoming => DeliveryStatus::Sent,
            MessageDirection::Outgoing => DeliveryStatus::RefusedNoTransport,
        };
        self.post_message_with_status(thread_id, dir, body, status)
    }

    /// Posts a message to a thread with explicit delivery status.
    pub fn post_message_with_status(
        &mut self,
        thread_id: u32,
        dir: MessageDirection,
        body: &[u8],
        status: DeliveryStatus,
    ) -> Option<u32> {
        let idx = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id)?;
        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        if self.next_msg_id == 0 {
            self.next_msg_id = 1;
        }

        self.virtual_clock = self.virtual_clock.saturating_add(1);
        let blen = body.len().min(MAX_MESSAGE_BODY_LEN);
        let mut rec = MessageRecord::EMPTY;
        rec.id = msg_id;
        rec.thread_id = thread_id;
        rec.timestamp = self.virtual_clock;
        rec.direction = dir;
        rec.status = status;
        rec.body[..blen].copy_from_slice(&body[..blen]);
        rec.body_len = blen;

        self.rings[idx].push(rec);
        self.threads[idx].last_timestamp = rec.timestamp;
        self.threads[idx].message_count = self.threads[idx].message_count.saturating_add(1);
        if dir == MessageDirection::Incoming {
            self.threads[idx].unread_count = self.threads[idx].unread_count.saturating_add(1);
        }
        Some(msg_id)
    }

    /// Fills `out` with the thread's messages ordered chronologically (oldest to newest).
    /// Returns the count of messages retrieved.
    pub fn get_thread_messages(&self, thread_id: u32, out: &mut [MessageRecord]) -> usize {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            self.rings[idx].get_messages(out)
        } else {
            0
        }
    }

    /// Marks a thread as read by resetting its unread counter to 0.
    pub fn mark_thread_read(&mut self, thread_id: u32) -> bool {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            self.threads[idx].unread_count = 0;
            true
        } else {
            false
        }
    }

    /// Returns a reference to the ThreadRecord for the given thread_id.
    pub fn get_thread(&self, thread_id: u32) -> Option<&ThreadRecord> {
        self.threads[..self.thread_count].iter().find(|t| t.id == thread_id)
    }

    /// Returns a reference to the ThreadRecord by index.
    pub fn get_thread_by_idx(&self, idx: usize) -> Option<&ThreadRecord> {
        if idx < self.thread_count {
            Some(&self.threads[idx])
        } else {
            None
        }
    }

    /// Returns the total unread messages count across all active threads.
    pub fn unread_total(&self) -> u32 {
        let mut sum = 0u32;
        for i in 0..self.thread_count {
            sum += self.threads[i].unread_count as u32;
        }
        sum
    }

    /// Removes a thread and its message ring from the ledger.
    pub fn remove_thread(&mut self, thread_id: u32) -> bool {
        if let Some(idx) = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id) {
            for i in idx..self.thread_count - 1 {
                self.threads[i] = self.threads[i + 1];
                self.rings[i] = self.rings[i + 1];
            }
            self.threads[self.thread_count - 1] = ThreadRecord::EMPTY;
            self.rings[self.thread_count - 1] = ThreadMessageRing::EMPTY;
            self.thread_count -= 1;
            true
        } else {
            false
        }
    }

    /// Returns the latest message for a thread.
    pub fn latest_message(&self, thread_id: u32) -> Option<&MessageRecord> {
        let idx = self.threads[..self.thread_count].iter().position(|t| t.id == thread_id)?;
        self.rings[idx].latest()
    }
}

/// Legacy conversation thread record retained for compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationThread {
    pub sender: [u8; MAX_PHONE_LEN],
    pub sender_len: usize,
    pub last_snippet: [u8; MAX_SNIPPET_LEN],
    pub snippet_len: usize,
    pub unread_count: u8,
    pub last_timestamp_secs: u64,
}

impl ConversationThread {
    pub const EMPTY: Self = Self {
        sender: [0; MAX_PHONE_LEN],
        sender_len: 0,
        last_snippet: [0; MAX_SNIPPET_LEN],
        snippet_len: 0,
        unread_count: 0,
        last_timestamp_secs: 0,
    };

    pub fn new(sender_bytes: &[u8], snippet: &[u8], timestamp: u64) -> Self {
        let mut t = Self::EMPTY;
        let s = sender_bytes.len().min(MAX_PHONE_LEN);
        t.sender[..s].copy_from_slice(&sender_bytes[..s]);
        t.sender_len = s;
        let sn = snippet.len().min(MAX_SNIPPET_LEN);
        t.last_snippet[..sn].copy_from_slice(&snippet[..sn]);
        t.snippet_len = sn;
        t.last_timestamp_secs = timestamp;
        t
    }

    pub fn update_snippet(&mut self, snippet: &[u8], timestamp: u64, is_incoming: bool) {
        let sn = snippet.len().min(MAX_SNIPPET_LEN);
        self.last_snippet = [0; MAX_SNIPPET_LEN];
        self.last_snippet[..sn].copy_from_slice(&snippet[..sn]);
        self.snippet_len = sn;
        self.last_timestamp_secs = timestamp;
        if is_incoming {
            self.unread_count = self.unread_count.saturating_add(1);
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incoming_record_says_it_was_written_on_this_device() {
        // WHY this test exists: the record used to be drawn as "Received
        // (no transport)". Nothing can arrive, because no transport is bound
        // to this app, so a record whose direction is incoming can only be
        // one this device wrote. A label that says it was received is a
        // claim about a channel the app has never had.
        let label = INCOMING_LOCAL_LABEL;
        assert!(!label.windows(8).any(|w| w == b"Received"));
        assert!(!label.windows(6).any(|w| w == b"inbox"));
        assert!(label.windows(7).any(|w| w == b"locally"));
        assert!(label.windows(7).any(|w| w == b"never r"));
    }

    #[test]
    fn a_hidden_preview_shows_the_message_itself() {
        // WHY this test exists: the privacy setting is only a privacy
        // control if the preview is not on the screen. A preview that was
        // merely shortened would still be the message.
        assert_eq!(HIDDEN_PREVIEW_LABEL, b"Message");
    }

    #[test]
    fn a_filtered_list_is_addressed_by_the_matching_row() {
        // WHY this test exists: the list draws only the rows that match, so
        // the row under the finger is the n-th match and not the n-th
        // thread. Addressing it by the ledger's own index would open the
        // wrong conversation and delete the wrong thread.
        let mut ledger = ThreadLedger::new();
        ledger.create_or_get_thread(b"+15550000");
        ledger.create_or_get_thread(b"+15551111");
        ledger.create_or_get_thread(b"+15552222");

        // An empty query matches everything, so the row and the ledger
        // agree.
        assert_eq!(
            filtered_thread_index(&ledger.threads, ledger.thread_count, b"", 0),
            Some(0)
        );
        assert_eq!(
            filtered_thread_index(&ledger.threads, ledger.thread_count, b"", 2),
            Some(2)
        );

        // A query that matches one thread reports that thread's own index.
        assert_eq!(
            filtered_thread_index(&ledger.threads, ledger.thread_count, b"2222", 0),
            Some(2)
        );
        assert_eq!(
            filtered_thread_index(&ledger.threads, ledger.thread_count, b"2222", 1),
            None
        );
    }

    #[test]
    fn a_delivery_state_never_claims_a_transport_it_did_not_have() {
        // WHY this test exists: the delivery state used to be derived from a
        // hardware-presence query, so a present modem produced "Sent"
        // without a message ever being handed to it. A state that says
        // "Pending" is also a claim -- it says a queue exists -- and no
        // queue exists.
        assert_eq!(DeliveryStatus::Sent.as_str(), "Sent");
        assert_eq!(DeliveryStatus::Failed.as_str(), "Failed");
        assert_eq!(
            DeliveryStatus::RefusedNoTransport.as_str(),
            "Refused - No Transport"
        );
        assert_eq!(DeliveryStatus::LocalOnly.as_str(), "Local only - not sent");
        assert_eq!(DeliveryStatus::Pending.as_str(), "Not sent");
        assert_ne!(DeliveryStatus::Pending.as_str(), "Pending");
    }

    #[test]
    fn a_name_or_a_handle_is_not_something_a_modem_can_dial() {
        // WHY this test exists: a recipient that is a name must not be
        // handed to a transport. The message stays local, and the app says
        // so, instead of reporting a transmission that never happened.
        assert!(is_dialable(b"+441234567890"));
        assert!(is_dialable(b"555 010 1234"));
        assert!(is_dialable(b"555-010-1234"));
        assert!(!is_dialable(b""));
        assert!(!is_dialable(b"Alice"));
        assert!(!is_dialable(b"alice@example"));
        assert!(!is_dialable(b"555 CALL"));
    }

    #[test]
    fn test_message_draft_limits() {
        let mut draft = MessageDraft::new();
        draft.set_recipient(b"+1234567890");
        assert_eq!(&draft.recipient[..draft.recipient_len], b"+1234567890");
        assert_eq!(draft.remaining_chars(), MAX_DRAFT_LEN);

        for _ in 0..MAX_DRAFT_LEN {
            assert!(draft.push_char(b'A'));
        }
        assert_eq!(draft.remaining_chars(), 0);
        assert!(!draft.push_char(b'B'));

        assert_eq!(draft.pop_char(), Some(b'A'));
        assert_eq!(draft.remaining_chars(), 1);
        draft.clear();
        assert_eq!(draft.len, 0);
    }

    #[test]
    fn test_thread_ledger_management() {
        let mut ledger = ThreadLedger::new();
        assert_eq!(ledger.thread_count, 0);
        assert_eq!(ledger.unread_total(), 0);

        let t1_id = ledger.create_or_get_thread(b"+15550001").unwrap();
        ledger.post_message(t1_id, MessageDirection::Incoming, b"Hello there!");
        assert_eq!(ledger.unread_total(), 1);
        assert_eq!(ledger.get_thread(t1_id).unwrap().unread_count, 1);

        let t2_id = ledger.create_or_get_thread(b"+15550002").unwrap();
        ledger.post_message(t2_id, MessageDirection::Incoming, b"Meeting at 3pm");
        ledger.post_message(t2_id, MessageDirection::Incoming, b"See you soon");
        assert_eq!(ledger.get_thread(t2_id).unwrap().unread_count, 2);

        assert_eq!(ledger.thread_count, 2);
        assert_eq!(ledger.unread_total(), 3);

        // Re-finding existing returns same record id
        let t1_refound_id = ledger.create_or_get_thread(b"+15550001").unwrap();
        assert_eq!(t1_refound_id, t1_id);

        // Mark thread 2 read
        assert!(ledger.mark_thread_read(t2_id));
        assert_eq!(ledger.get_thread(t2_id).unwrap().unread_count, 0);
        assert_eq!(ledger.unread_total(), 1);
    }

    #[test]
    fn test_thread_ledger_circular_overflow() {
        let mut ledger = ThreadLedger::new();
        let tid = ledger.create_or_get_thread(b"+19998887777").unwrap();

        // Post 20 messages (ring buffer capacity is 16)
        for i in 0..20u8 {
            let body = [b'A' + i; 10];
            ledger.post_message(tid, MessageDirection::Outgoing, &body);
        }

        let mut retrieved = [MessageRecord::EMPTY; 20];
        let count = ledger.get_thread_messages(tid, &mut retrieved);
        assert_eq!(count, 16);

        // The oldest 4 messages (0..3, 'A'..'D') must have been dropped.
        // First returned message must be 'E' (i=4)
        assert_eq!(retrieved[0].body[0], b'A' + 4);
        // Last returned message must be 'T' (i=19)
        assert_eq!(retrieved[15].body[0], b'A' + 19);

        // Thread message_count tracks total posted (20)
        assert_eq!(ledger.get_thread(tid).unwrap().message_count, 20);
    }

    #[test]
    fn test_thread_ledger_max_threads_capacity() {
        let mut ledger = ThreadLedger::new();
        for i in 0..8 {
            let handle = [b'0' + i as u8; 4];
            assert!(ledger.create_or_get_thread(&handle).is_some());
        }
        assert_eq!(ledger.thread_count, 8);
        // 9th thread must be refused
        assert!(ledger.create_or_get_thread(b"9999").is_none());
    }

    #[test]
    fn test_delivery_status_truthful_refused_no_transport() {
        let mut ledger = ThreadLedger::new();
        let tid = ledger.create_or_get_thread(b"+15550099").unwrap();
        ledger.post_message(tid, MessageDirection::Outgoing, b"Hello from isolated sandbox");

        let mut msgs = [MessageRecord::EMPTY; 4];
        let n = ledger.get_thread_messages(tid, &mut msgs);
        assert_eq!(n, 1);
        assert_eq!(msgs[0].direction, MessageDirection::Outgoing);
        assert_eq!(msgs[0].status, DeliveryStatus::RefusedNoTransport);
        assert_eq!(msgs[0].status.as_str(), "Refused - No Transport");
    }

    #[test]
    fn test_messages_ui_text_bounds_and_ellipsize() {
        use interos_std::design::font;
        let mut buf = [0u8; 96];
        let long_val = b"/system/state/secure_storage/messages_diagnostic_and_audit_log_record.json";
        let card_w = 672;
        let max_w = card_w - 48; // 624
        let len = font::ellipsize(long_val, 2, max_w, &mut buf);
        let disp = if len > 0 { &buf[..len] } else { long_val.as_slice() };
        assert!(font::measure(disp, 2) <= max_w);
        assert!(disp.ends_with(b"...") || disp == long_val.as_slice());

        // Recipient / placeholder messages fit within horizontal bounds
        for s in [INCOMING_LOCAL_LABEL, HIDDEN_PREVIEW_LABEL, b"Nothing has been published or shared yet.".as_slice()] {
            assert!(font::measure(s, 1) <= max_w);
        }
    }

    /// The send control's glyph has to read as "go", not as "back".
    ///
    /// WHY this asks the predicate rather than the screen: the arrow is drawn
    /// from two composer layouts and used to be written out twice, both times
    /// with its barbs widening away from the tip, which is a left arrow. The
    /// direction is now one function and this is where it is checked.
    #[test]
    fn the_send_arrow_points_forward_out_of_the_circle() {
        let inked = |dy: isize| -> std::vec::Vec<isize> {
            (-29isize..=29).filter(|dx| send_arrow_ink(*dx, dy)).collect()
        };

        let centre = inked(0);
        assert_eq!(centre.last(), Some(&8), "the vertex is not on the centre row");
        assert_eq!(
            inked(-8).last(),
            inked(8).last(),
            "the barbs are not symmetric about the centre row"
        );

        // Off the shaft the ink must recede to the left: barbs that open away
        // from the tip are what make this a '>' rather than a '<'.
        for dy in 3..=8 {
            let off = inked(dy);
            assert!(!off.is_empty(), "the barb stops before the circle's edge");
            assert!(
                off.last() < centre.last(),
                "the barb at dy={dy} reaches {:?}, past the vertex",
                off.last()
            );
        }

        for dy in -29..=29 {
            for dx in inked(dy) {
                assert!(dx * dx + dy * dy <= 29 * 29, "ink escapes the control");
            }
        }
    }
}

