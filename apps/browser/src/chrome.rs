//! The boxes the browser paints its chrome controls in, and answers taps in.
//!
//! WHY this is a module and not a number at each call site: the renderer walks
//! the panel one pixel at a time and the press path answers one press at a
//! time, and for most of this app's life each carried its own copy of the
//! rectangle. Every copy was a way for the two to part, and the parting is the
//! defect the owner sees -- a mark on the screen that answers nothing, and a
//! stretch of blank panel that opens something. Each box here is read by the
//! painter and by the press path, so neither can claim an edge the other has
//! not drawn, and the gates under each box are the platform's [`MIN_TARGET`]
//! floor applied to the very constants the painter uses.
//!
//! WHY the boxes carry the sheet's and the field's own origins (the header
//! band, the filter row, the accessory bar): a control whose plate is placed by
//! one rule and whose band is placed by another is the same defect one level
//! down. The origins live beside the boxes they contain, so moving a row moves
//! its control.

use interos_std::design::primitives::Rect;
use interos_std::design::touch::MIN_TARGET;
use interos_std::design::viewport::LOGICAL_WIDTH;

/// The panel width every box is measured against.
///
/// WHY restated: the renderer owns its own `W`, and a control placed from a
/// width the two files disagree about lands off the edge of the screen. The
/// gate in `main.rs` binds this to the renderer's constant at compile time.
pub const PANEL_W: usize = LOGICAL_WIDTH;

/// The gutter every sheet card and list row keeps from the panel's edge.
pub const SIDE_GUTTER: usize = 24;

/// The card's own width between the two gutters.
pub const CARD_W: usize = PANEL_W - 2 * SIDE_GUTTER;

/// The sheet header's top row, and how tall the header band is.
///
/// WHY both: every sheet draws its title and its close control in this band,
/// and the control has to be inside the strip the sheet actually paints.
pub const SHEET_HEADER_Y0: usize = 120;
pub const SHEET_HEADER_H: usize = 50;

/// The close control a sheet wears in the head of its header band.
///
/// WHY a plate at all: the sheets drew a bare 17-row `X` and answered a press
/// anywhere in a 56x46 strip whose left edge was 12 columns left of the ink and
/// whose top was 17 rows above it, so the person aimed at a glyph they could
/// see and pressed a region of empty header. The plate is the platform's own
/// chrome tile, drawn at the floor a touch target may not go under, and the
/// press answers exactly that plate.
pub const SHEET_CLOSE: Rect = Rect::new(
    PANEL_W - SIDE_GUTTER - MIN_TARGET,
    SHEET_HEADER_Y0 + (SHEET_HEADER_H - MIN_TARGET) / 2,
    MIN_TARGET,
    MIN_TARGET,
);

const _: () = assert!(SHEET_CLOSE.w >= MIN_TARGET && SHEET_CLOSE.h >= MIN_TARGET);
const _: () = assert!(SHEET_CLOSE.y >= SHEET_HEADER_Y0);
const _: () = assert!(SHEET_CLOSE.y + SHEET_CLOSE.h <= SHEET_HEADER_Y0 + SHEET_HEADER_H);
const _: () = assert!(SHEET_CLOSE.x + SHEET_CLOSE.w <= PANEL_W);

/// The top of the filter row in the bookmark and history sheets.
pub const FILTER_Y: usize = 176;

/// The height of that row's card.
///
/// WHY taller than the platform's floor: the clear control has to be the floor,
/// and a 44-row plate set flush against a card that is itself 44 rows tall paints
/// its own corners outside the card's rounding, over the content the sheet is
/// dimming. Eight rows of card on either side of the plate is what keeps the
/// control inside the thing it belongs to.
pub const FILTER_H: usize = 52;

/// The gutter the clear control keeps from the card's trailing edge.
const FILTER_CLEAR_GUTTER: usize = 8;

/// The filter row's card.
pub const FILTER_CARD: Rect = Rect::new(SIDE_GUTTER, FILTER_Y, CARD_W, FILTER_H);

/// The filter's clear control, at the trailing edge of that card.
///
/// WHY it is a plate at all: the row drew a bare 17-row `X` and only when there
/// was something to clear, while the press answered 73 columns of empty card --
/// so half the band people pressed had no control in it, and the ink they aimed
/// at had no edges. Now the plate is drawn where the press answers, and only when
/// it has something to do.
pub const FILTER_CLEAR: Rect = Rect::new(
    PANEL_W - SIDE_GUTTER - FILTER_CLEAR_GUTTER - MIN_TARGET,
    FILTER_Y + (FILTER_H - MIN_TARGET) / 2,
    MIN_TARGET,
    MIN_TARGET,
);

const _: () = assert!(FILTER_CLEAR.w >= MIN_TARGET && FILTER_CLEAR.h >= MIN_TARGET);
const _: () = assert!(FILTER_CLEAR.y > FILTER_Y);
const _: () = assert!(FILTER_CLEAR.y + FILTER_CLEAR.h < FILTER_Y + FILTER_H);
const _: () = assert!(FILTER_CLEAR.x + FILTER_CLEAR.w < FILTER_CARD.x + FILTER_CARD.w);

/// The rows of the sheet lists, and the card each row is drawn in.
///
/// WHY named here: three sheets drew their card `W - 48` wide and then answered
/// a press with `x <= W - 24`, which is the column just past the card's own
/// edge -- the press path claimed a column no pixel of ink was ever painted in.
pub const LIST_ROW_CARD_X: usize = SIDE_GUTTER;
pub const LIST_ROW_CARD_W: usize = CARD_W;

/// The list-row card the press answers, at row top `top` and `h` rows tall.
pub const fn list_row_card(top: usize, h: usize) -> Rect {
    Rect::new(LIST_ROW_CARD_X, top, LIST_ROW_CARD_W, h)
}

const _: () = assert!(LIST_ROW_CARD_W == PANEL_W - 2 * SIDE_GUTTER);

/// The rows the address field's suggestion cards are drawn in.
///
/// WHY the pair: the press path tested `130..=198` and `208..=276` against cards
/// painted 68 rows tall from 130 and from 208, so both bands claimed the first
/// row of the gap under their card, and both tested `x <= W - 24` against a card
/// that stops at that column.
pub const SUGGEST_CARD_H: usize = 68;
pub const SUGGEST_SEARCH: Rect = Rect::new(SIDE_GUTTER, 130, CARD_W, SUGGEST_CARD_H);
pub const SUGGEST_NAVIGATE: Rect = Rect::new(SIDE_GUTTER, 208, CARD_W, SUGGEST_CARD_H);

const _: () = assert!(SUGGEST_SEARCH.h >= MIN_TARGET && SUGGEST_SEARCH.w >= MIN_TARGET);
const _: () = assert!(SUGGEST_SEARCH.y + SUGGEST_SEARCH.h <= SUGGEST_NAVIGATE.y);
const _: () = assert!(SUGGEST_NAVIGATE.y + SUGGEST_NAVIGATE.h <= ACC_BAR_Y0);

/// The height of the action button the error, download and dismissal screens
/// draw under their status badge.
///
/// WHY 48 and not the 36 the badge is: the badge is a label, the action is a
/// control, and a control may not be sized under [`MIN_TARGET`].
pub const ACTION_H: usize = 48;

/// The action button's width, shared by the four screens that wear one.
pub const ACTION_W: usize = 180;

/// How far below the status badge a screen's action button stands.
pub const ACTION_DY: usize = 54;

/// The reload action of a failed navigation.
///
/// WHY derived from the badge's row: the badge sits below wrapped body text, so
/// its row moves with the length of the message. The press path used two fixed
/// bands, `630..=720` and `920..=968`, which matched the painted button only for
/// the one body length that happens to land there: a short message put the
/// button's top rows outside the band, a long one drew the button entirely below
/// it -- Reload Page visible and completely dead -- and `920..=968` answered a
/// reload across 90 rows of empty panel where nothing is drawn at all.
pub const fn retry_rect(err_y: usize) -> Rect {
    Rect::new(PANEL_W / 2 - ACTION_W / 2, err_y + ACTION_DY, ACTION_W, ACTION_H)
}

const _: () = assert!(ACTION_W >= MIN_TARGET && ACTION_H >= MIN_TARGET);
const _: () = assert!(retry_rect(SHEET_HEADER_Y0).x + ACTION_W <= PANEL_W);

/// The row of tiles the start page offers under "Quick links".
///
/// WHY the block's top is named too: the heading is drawn above the plates, and
/// the press path must not answer a tile in the heading's rows.
pub const HOME_GRID_Y0: usize = 252;
pub const HOME_TILE_Y: usize = 278;

/// The tile's plate, and the caption the tile is read with.
pub const HOME_TILE_SIZE: usize = 64;
pub const HOME_TILE_CAPTION_Y: usize = 350;

/// The first row past the grid block, and so the bottom of a tile's band.
///
/// WHY the band reaches past the plate: the caption is part of the tile -- it is
/// the only thing that says what the tile opens -- and a press on the word
/// "Wikipedia" used to do nothing at all while a press on the icon did.
pub const HOME_GRID_Y1: usize = 376;

/// The columns a tile answers: the whole slice of the strip between its two
/// neighbours, plate and caption together.
///
/// WHY the pitch decides it and not a slop typed under the plate: the caption is
/// part of the control -- it is the only thing that says what the tile opens --
/// and it runs wider than the plate it names. Measured against the typeface,
/// "YouTube" ends nine columns past a band drawn eight columns wider than its
/// plate, so those nine columns of a word the person is aiming at answered
/// nothing. Taking the half-gap each side of the plate gives every tile the
/// columns up to its neighbour's edge, which is what a phone's icon row actually
/// is: no dead strip between two icons, and never one column answered twice.
const TILE_SLOP: usize = (HOME_TILE_X[1] - HOME_TILE_X[0] - HOME_TILE_SIZE) / 2;

/// The plate column of each of the four tiles.
pub const HOME_TILE_X: [usize; 4] = [66, 234, 402, 570];

/// The column each tile's caption starts at, and the caption itself.
///
/// WHY the caption and its column sit beside the plate's column: the caption is
/// the only thing that says what a tile opens, so it is part of the control, not
/// decoration next to it. The band under `home_tile_hit` answers it, and the gate
/// after the table proves the ink is inside the band that answers it.
///
/// WHY "Wikipedia" starts four columns left of its own plate: the caption is set
/// centred under the plate it names, and the band reaches the same eight columns
/// on either side of every plate, so the ink it starts outside still lands on
/// this tile's band -- never its neighbour's, which the non-overlap gate holds.
pub const HOME_TILE_CAPTION_X: [usize; 4] = [70, 238, 410, 566];
pub const HOME_TILE_CAPTION: [&[u8]; 4] = [b"InterOS", b"YouTube", b"GitHub", b"Wikipedia"];

/// The address each tile opens, in the same order as its caption.
///
/// WHY the pair is one table: a caption that names a different page than the one
/// its press opens is a control that lies, and the two orders used to live in
/// different files -- the painter's text here, the press path's address there.
pub const HOME_TILE_URL: [&[u8]; 4] = [
    b"https://interos.tech",
    b"https://m.youtube.com",
    b"https://github.com",
    b"https://wikipedia.org",
];

/// The band that answers tile `i`: its plate and its caption.
pub const fn home_tile_hit(i: usize) -> Rect {
    Rect::new(
        HOME_TILE_X[i] - TILE_SLOP,
        HOME_TILE_Y,
        HOME_TILE_SIZE + 2 * TILE_SLOP,
        HOME_GRID_Y1 - HOME_TILE_Y,
    )
}

const _: () = assert!(home_tile_hit(0).w >= MIN_TARGET && home_tile_hit(0).h >= MIN_TARGET);
const _: () = assert!(HOME_GRID_Y0 < HOME_TILE_Y);
const _: () = assert!(HOME_TILE_Y >= SHEET_HEADER_Y0);
const _: () = assert!(HOME_TILE_CAPTION_Y >= HOME_TILE_Y + HOME_TILE_SIZE);
const _: () = assert!(HOME_TILE_CAPTION_Y < HOME_GRID_Y1);
const _: () = assert!(HOME_GRID_Y1 - HOME_TILE_Y >= HOME_TILE_SIZE);
const _: () = assert!(home_tile_hit(3).x + home_tile_hit(3).w <= PANEL_W);
const _: () = assert!(
    home_tile_hit(0).x + home_tile_hit(0).w <= home_tile_hit(1).x
        && home_tile_hit(1).x + home_tile_hit(1).w <= home_tile_hit(2).x
        && home_tile_hit(2).x + home_tile_hit(2).w <= home_tile_hit(3).x,
    "two tiles must never answer the same column",
);
const _: () = assert!(
    {
        let mut i = 0;
        let mut ok = true;
        while i < 4 {
            let hit = home_tile_hit(i);
            if HOME_TILE_CAPTION_X[i] < hit.x {
                ok = false;
            }
            if HOME_TILE_CAPTION_X[i] > HOME_TILE_X[i] + HOME_TILE_SIZE {
                ok = false;
            }
            i += 1;
        }
        ok
    },
    "a tile's caption must start inside the band that answers it, and inside the
     plate's own columns or the slop beside them",
);

/// The top of the accessory strip the keyboard dock carries.
pub const ACC_BAR_Y0: usize = 918;

/// The height of that strip's own plate.
pub const ACC_BAR_H: usize = 56;

/// How far inside the strip a chip stands, and so the chip's top row.
const ACC_CHIP_DY: usize = 6;

/// The height of every chip in the strip.
pub const ACC_CHIP_H: usize = MIN_TARGET;

/// The chip's top row, read by the strip's painter and its press path alike.
pub const ACC_CHIP_Y: usize = ACC_BAR_Y0 + ACC_CHIP_DY;

/// The bottom row of the strip's plate, exclusive.
pub const ACC_BAR_Y1: usize = ACC_BAR_Y0 + ACC_BAR_H;

/// The address chip: the one thing in the strip that is not a name-completer.
pub const ACC_HTTPS: Rect = Rect::new(24, ACC_CHIP_Y, 110, ACC_CHIP_H);
pub const ACC_TLD_COM: Rect = Rect::new(144, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_TLD_ORG: Rect = Rect::new(234, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_CLEAR: Rect = Rect::new(324, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_DISMISS: Rect = Rect::new(630, ACC_CHIP_Y, 64, ACC_CHIP_H);

/// The two chips the strip wears while a page's own field is focused.
pub const FIELD_CLEAR: Rect = Rect::new(24, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const FIELD_DONE: Rect = Rect::new(600, ACC_CHIP_Y, 94, ACC_CHIP_H);

const _: () = assert!(ACC_HTTPS.w >= MIN_TARGET && ACC_HTTPS.h >= MIN_TARGET);
const _: () = assert!(ACC_TLD_COM.w >= MIN_TARGET && ACC_TLD_ORG.w >= MIN_TARGET);
const _: () = assert!(ACC_CLEAR.w >= MIN_TARGET && ACC_DISMISS.w >= MIN_TARGET);
const _: () = assert!(FIELD_CLEAR.w >= MIN_TARGET && FIELD_DONE.w >= MIN_TARGET);
const _: () = assert!(ACC_CHIP_Y + ACC_CHIP_H <= ACC_BAR_Y1);
const _: () = assert!(ACC_HTTPS.x >= SIDE_GUTTER);
const _: () = assert!(ACC_DISMISS.x + ACC_DISMISS.w <= PANEL_W - SIDE_GUTTER);
const _: () = assert!(FIELD_DONE.x + FIELD_DONE.w <= PANEL_W - SIDE_GUTTER);

#[cfg(test)]
mod tests {
    use super::*;
    use interos_std::design::back_button;
    use interos_std::design::touch::MIN_TARGET;

    /// Every control box in this module clears the platform's touch floor.
    ///
    /// WHY a table and not one test per box: the floor is one rule, and a rule
    /// checked in twelve places is twelve places that can be edited apart. A box
    /// that joins this module without joining this table is not gating the row
    /// the person is asked to press.
    #[test]
    fn every_chrome_control_clears_the_platform_touch_floor() {
        let boxes: [(&str, Rect); 15] = [
            ("sheet close", SHEET_CLOSE),
            ("filter card", FILTER_CARD),
            ("filter clear", FILTER_CLEAR),
            ("retry", retry_rect(SHEET_HEADER_Y0)),
            ("retry at the longest message", retry_rect(672)),
            ("suggestion search", SUGGEST_SEARCH),
            ("suggestion navigate", SUGGEST_NAVIGATE),
            ("list row", list_row_card(FILTER_Y + FILTER_H, 62)),
            ("accessory https", ACC_HTTPS),
            ("accessory .com", ACC_TLD_COM),
            ("accessory .org", ACC_TLD_ORG),
            ("accessory clear", ACC_CLEAR),
            ("accessory dismiss", ACC_DISMISS),
            ("field clear", FIELD_CLEAR),
            ("field done", FIELD_DONE),
        ];
        for (name, r) in boxes {
            assert!(r.w >= MIN_TARGET, "{name} is {} wide", r.w);
            assert!(r.h >= MIN_TARGET, "{name} is {} tall", r.h);
            assert!(r.x + r.w <= PANEL_W, "{name} overhangs the panel's edge");
        }
    }

    /// The four start-page tiles answer their plate and their caption, and
    /// never each other's columns.
    #[test]
    fn a_quick_link_tile_answers_its_plate_and_its_caption() {
        for i in 0..HOME_TILE_X.len() {
            let hit = home_tile_hit(i);
            // The plate the renderer draws sits inside the band, so no pixel of
            // the control is unreachable.
            assert!(
                hit.x <= HOME_TILE_X[i] && hit.x + hit.w >= HOME_TILE_X[i] + HOME_TILE_SIZE,
                "tile {i} does not answer its own plate"
            );
            assert!(
                hit.y <= HOME_TILE_Y && hit.y + hit.h > HOME_TILE_CAPTION_Y,
                "tile {i} does not answer the caption that names it"
            );
            assert!(hit.w >= MIN_TARGET && hit.h >= MIN_TARGET, "tile {i}");
        }
        for i in 1..HOME_TILE_X.len() {
            let prev = home_tile_hit(i - 1);
            assert!(prev.x + prev.w <= home_tile_hit(i).x);
        }
    }

    /// A tile's caption is answered across its whole run, not just its first
    /// column.
    ///
    /// WHY measured against the face: the const gate beside the table can only
    /// see where a caption starts. How far it runs is the typeface's business,
    /// and a word that ran past its band would be a label the neighbouring tile
    /// answers -- the painted-versus-tappable defect wearing a second hat.
    #[test]
    fn a_quick_link_caption_is_answered_across_its_whole_run() {
        for i in 0..HOME_TILE_X.len() {
            let hit = home_tile_hit(i);
            let run = interos_std::design::font::measure(HOME_TILE_CAPTION[i], 1);
            assert!(run > 0, "tile {i} paints no caption at all");
            assert!(
                HOME_TILE_CAPTION_X[i] + run <= hit.x + hit.w,
                "tile {i}'s caption runs past its band by {} columns",
                HOME_TILE_CAPTION_X[i] + run - (hit.x + hit.w)
            );
            assert!(
                HOME_TILE_CAPTION_X[i] >= hit.x,
                "tile {i}'s caption starts outside the band that answers it"
            );
        }
    }

    /// The reload action answers exactly the plate the error screen draws.
    ///
    /// WHY swept over every badge row: the badge's row is a function of how many
    /// lines the failure message wrapped to, and the defect this gate holds shut
    /// was a fixed band that answered only one of those rows.
    #[test]
    fn the_reload_action_answers_the_plate_at_every_message_length() {
        for body_n in 1..=6usize {
            let err_y = 440 + 40 + body_n * 28 + 24;
            let retry = retry_rect(err_y);
            assert_eq!(retry.y, err_y + ACTION_DY);
            assert_eq!(retry.h, ACTION_H);
            // A press on any row of the drawn button is answered...
            assert!(retry.contains(retry.x, retry.y));
            assert!(retry.contains(retry.x + retry.w - 1, retry.y + retry.h - 1));
            // ...and a press just outside it is not.
            assert!(!retry.contains(retry.x, retry.y - 1));
            assert!(!retry.contains(retry.x, retry.y + retry.h));
            assert!(!retry.contains(retry.x - 1, retry.y));
            assert!(!retry.contains(retry.x + retry.w, retry.y));
        }
    }

    /// Every sheet's close control is drawn where it answers.
    #[test]
    fn the_sheet_close_control_is_painted_where_it_answers() {
        assert!(!SHEET_CLOSE.contains(back_button::X, back_button::Y));
        // The plate is the SDK's own chrome tile: 44 rows inside a 50-row
        // header band, so the band cannot be edited out from under it.
        assert_eq!(SHEET_CLOSE.w, MIN_TARGET);
        assert_eq!(SHEET_CLOSE.h, MIN_TARGET);
        assert_eq!(
            SHEET_CLOSE.x + SHEET_CLOSE.w,
            PANEL_W - SIDE_GUTTER,
            "the close control keeps the sheet's own gutter"
        );
        for y in SHEET_CLOSE.y..SHEET_CLOSE.y + SHEET_CLOSE.h {
            assert!(y >= SHEET_HEADER_Y0 && y < SHEET_HEADER_Y0 + SHEET_HEADER_H);
        }
    }

    /// The filter's clear control answers its plate and none of the card's empty
    /// padding, and stays inside the card it is drawn on.
    #[test]
    fn the_filter_clear_answers_its_own_plate_only() {
        assert_eq!(FILTER_CLEAR.w, MIN_TARGET);
        assert_eq!(FILTER_CLEAR.h, MIN_TARGET);
        // Trailing half of the card, which is where a clear control belongs.
        assert!(FILTER_CLEAR.x > FILTER_CARD.x + FILTER_CARD.w / 2);
        // Card left around the plate: its own rounding, plus the gutter.
        assert!(FILTER_CARD.x + FILTER_CARD.w - (FILTER_CLEAR.x + FILTER_CLEAR.w) >= 8);
        assert!(FILTER_CLEAR.y - FILTER_CARD.y >= 4);
        assert!(FILTER_CARD.y + FILTER_CARD.h - (FILTER_CLEAR.y + FILTER_CLEAR.h) >= 4);
    }

    /// The accessory strip's chips tile its plate without overlapping, and the
    /// strip's own band is the plate the renderer fills.
    #[test]
    fn the_accessory_chips_answer_their_plates_and_not_the_strip() {
        let chips = [
            &ACC_HTTPS,
            &ACC_TLD_COM,
            &ACC_TLD_ORG,
            &ACC_CLEAR,
            &ACC_DISMISS,
        ];
        for chip in chips {
            assert_eq!(chip.y, ACC_CHIP_Y);
            assert_eq!(chip.h, ACC_CHIP_H);
            assert!(chip.y + chip.h <= ACC_BAR_Y1);
        }
        assert!(ACC_HTTPS.x + ACC_HTTPS.w <= ACC_TLD_COM.x);
        assert!(ACC_TLD_COM.x + ACC_TLD_COM.w <= ACC_TLD_ORG.x);
        assert!(ACC_TLD_ORG.x + ACC_TLD_ORG.w <= ACC_CLEAR.x);
        assert!(ACC_CLEAR.x + ACC_CLEAR.w <= ACC_DISMISS.x);

        // The focused-field pair sits in the same band and clears each other.
        assert!(FIELD_CLEAR.x + FIELD_CLEAR.w <= FIELD_DONE.x);

        // The strip's plate ends where the band tested by the press path ends,
        // so the row under the plate is not the strip's to answer.
        assert_eq!(ACC_BAR_Y1, ACC_BAR_Y0 + ACC_BAR_H);
        assert!(!ACC_HTTPS.contains(ACC_HTTPS.x, ACC_BAR_Y1));
    }
}
