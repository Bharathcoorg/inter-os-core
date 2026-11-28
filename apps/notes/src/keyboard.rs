// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The soft keyboard's keys.
//!
//! WHY the keys are a table and not a picture of hardcoded rectangles: the
//! sheet has to be scrollable, and a sheet that scrolls has to know how
//! tall its content is before it draws it. A table of keys gives the
//! scroll model a height to work with, and it gives the renderer and the
//! tap handler one description of where the keys are, so a key cannot be
//! drawn in one place and pressed in another.
//!
//! WHY the symbols page is taller than the dock: a page of symbols needs
//! more rows than a page of letters, and the sheet scrolls when it has
//! more content than the dock can show. The letters page fits and does not
//! scroll, and the scroll model says so rather than pretending to scroll.
//!
//! WHY the plate of a key is declared once, in [`key_plate`]: the painter and
//! the press are two functions, and a keyboard whose press answers for rows
//! the sheet never inked is a keyboard that types letters nobody can see
//! under their thumb.

/// The platform's rectangle, so the plate the painter fills and the plate the
/// press answers are the same type the platform's controls are declared in.
pub use interos_std::design::primitives::Rect;

/// What a key of the soft keyboard does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyAction {
    /// Types a character.
    Char(u8),
    /// Turns the shift state on or off.
    Shift,
    /// Turns the symbols page on or off.
    Symbols,
    /// Deletes the character before the cursor.
    Backspace,
    /// Types a space.
    Space,
    /// Dismisses the keyboard.
    Done,
}

/// One key of the soft keyboard: where it sits and what it does.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    /// The key's left edge.
    pub x: usize,
    /// The key's width.
    pub w: usize,
    /// What the key does.
    pub action: KeyAction,
}

/// One row of the soft keyboard.
#[derive(Clone, Copy, Debug)]
pub struct KeyRow {
    /// The row's keys, left to right.
    pub keys: &'static [Key],
}

/// Builds a row of keys from `(x, width, action)` triples.
macro_rules! key_row {
    ($($x:expr, $w:expr, $a:expr);* $(;)?) => {
        KeyRow {
            keys: &[$(Key { x: $x, w: $w, action: $a }),*],
        }
    };
}

/// The keyboard dock's top edge.
pub const KB_Y0: usize = 980;
/// The keyboard dock's bottom edge.
pub const KB_Y1: usize = 1410;
/// The first key row's top edge.
pub const KB_ROW0_Y: usize = 994;
/// The distance from one key row's top to the next one's top.
pub const KB_ROW_STEP: usize = 82;
/// The height of a key.
pub const KB_KEY_H: usize = 72;
/// The width of a letter key.
pub const KB_KEY_W: usize = 60;
/// The gap between the dock's top and the first row.
pub const KB_TOP_MARGIN: usize = KB_ROW0_Y - KB_Y0;

/// The letters page: four rows that fit the dock.
pub const LETTER_ROWS: &[KeyRow] = &[
    key_row![
        15, 60, KeyAction::Char(b'Q');
        85, 60, KeyAction::Char(b'W');
        155, 60, KeyAction::Char(b'E');
        225, 60, KeyAction::Char(b'R');
        295, 60, KeyAction::Char(b'T');
        365, 60, KeyAction::Char(b'Y');
        435, 60, KeyAction::Char(b'U');
        505, 60, KeyAction::Char(b'I');
        575, 60, KeyAction::Char(b'O');
        645, 60, KeyAction::Char(b'P');
    ],
    key_row![
        46, 60, KeyAction::Char(b'A');
        116, 60, KeyAction::Char(b'S');
        186, 60, KeyAction::Char(b'D');
        256, 60, KeyAction::Char(b'F');
        326, 60, KeyAction::Char(b'G');
        396, 60, KeyAction::Char(b'H');
        466, 60, KeyAction::Char(b'J');
        536, 60, KeyAction::Char(b'K');
        606, 60, KeyAction::Char(b'L');
    ],
    key_row![
        15, 80, KeyAction::Shift;
        105, 60, KeyAction::Char(b'Z');
        175, 60, KeyAction::Char(b'X');
        245, 60, KeyAction::Char(b'C');
        315, 60, KeyAction::Char(b'V');
        385, 60, KeyAction::Char(b'B');
        455, 60, KeyAction::Char(b'N');
        525, 60, KeyAction::Char(b'M');
        620, 85, KeyAction::Backspace;
    ],
    key_row![
        15, 100, KeyAction::Symbols;
        125, 70, KeyAction::Char(b',');
        205, 310, KeyAction::Space;
        525, 70, KeyAction::Char(b'.');
        605, 100, KeyAction::Done;
    ],
];

/// The symbols page: six rows, so the sheet scrolls.
pub const SYMBOL_ROWS: &[KeyRow] = &[
    key_row![
        15, 60, KeyAction::Char(b'1');
        85, 60, KeyAction::Char(b'2');
        155, 60, KeyAction::Char(b'3');
        225, 60, KeyAction::Char(b'4');
        295, 60, KeyAction::Char(b'5');
        365, 60, KeyAction::Char(b'6');
        435, 60, KeyAction::Char(b'7');
        505, 60, KeyAction::Char(b'8');
        575, 60, KeyAction::Char(b'9');
        645, 60, KeyAction::Char(b'0');
    ],
    key_row![
        15, 60, KeyAction::Char(b'-');
        85, 60, KeyAction::Char(b'/');
        155, 60, KeyAction::Char(b':');
        225, 60, KeyAction::Char(b';');
        295, 60, KeyAction::Char(b'(');
        365, 60, KeyAction::Char(b')');
        435, 60, KeyAction::Char(b'$');
        505, 60, KeyAction::Char(b'&');
        575, 60, KeyAction::Char(b'@');
        645, 60, KeyAction::Char(b'"');
    ],
    key_row![
        105, 60, KeyAction::Char(b'?');
        175, 60, KeyAction::Char(b'!');
        245, 60, KeyAction::Char(b'\'');
        315, 60, KeyAction::Char(b'#');
        385, 60, KeyAction::Char(b'%');
        455, 60, KeyAction::Char(b'*');
        525, 60, KeyAction::Char(b'+');
    ],
    key_row![
        15, 60, KeyAction::Char(b',');
        85, 60, KeyAction::Char(b'.');
        155, 60, KeyAction::Char(b'`');
        225, 60, KeyAction::Char(b'~');
        295, 60, KeyAction::Char(b'^');
        365, 60, KeyAction::Char(b'_');
        435, 60, KeyAction::Char(b'|');
        505, 60, KeyAction::Char(b'\\');
    ],
    key_row![
        15, 60, KeyAction::Char(b'<');
        85, 60, KeyAction::Char(b'>');
        155, 60, KeyAction::Char(b'[');
        225, 60, KeyAction::Char(b']');
        295, 60, KeyAction::Char(b'{');
        365, 60, KeyAction::Char(b'}');
        435, 60, KeyAction::Char(b'+');
        505, 60, KeyAction::Char(b'=');
    ],
    key_row![
        15, 100, KeyAction::Symbols;
        205, 310, KeyAction::Space;
        605, 100, KeyAction::Done;
    ],
];

/// The rows of the page the keyboard is showing.
pub const fn rows(symbols: bool) -> &'static [KeyRow] {
    if symbols {
        SYMBOL_ROWS
    } else {
        LETTER_ROWS
    }
}

/// The height of the dock the sheet is drawn in.
pub const fn viewport_h() -> usize {
    KB_Y1 - KB_Y0
}

/// The height of the sheet's content.
///
/// WHY the content is taller than the rows: the first row starts below the
/// dock's top edge, so the content carries that margin too. A content
/// height that left the margin out would scroll the sheet one row too far.
pub const fn content_h(symbols: bool) -> usize {
    rows(symbols).len() * KB_ROW_STEP + KB_TOP_MARGIN
}

/// The top edge of a row, given the sheet's scroll offset.
pub const fn row_y(index: usize, offset: usize) -> usize {
    KB_ROW0_Y + index * KB_ROW_STEP - offset
}

/// The plate a key is painted on, and the plate a press on that key answers.
///
/// WHY the plate is declared once: the painter and the press are two
/// functions. While each held its own geometry the keyboard answered for rows
/// and columns the sheet never inked -- the painter drew every key as a pill
/// inset three columns from each side and twelve rows short at the bottom,
/// while the press answered the whole cell and then reached half a gutter
/// further into every gap. So the first column of the sheet answered 'q', the
/// row above the first key answered a letter nobody could see, and the
/// bottom of every key answered a key that was not drawn there.
///
/// WHY the plate is the cell and not the inset pill: the platform's own soft
/// keyboard paints a stadium pill that fills the key's rect and answers that
/// same rect, and a control that invents its own size is a control a
/// third-party app cannot match.
pub const fn key_plate(key: &Key, top: usize) -> Rect {
    Rect::new(key.x, top, key.w, KB_KEY_H)
}

/// The action of the key at `(x, y)`, if there is one.
///
/// WHY the offset is passed in and not read from the state: the renderer
/// and the tap handler have to agree on which key is under the finger, and
/// an offset each of them looked up separately is an offset that can
/// differ.
///
/// WHY the answer is the plate and nothing wider: a press answers the key
/// that is drawn under it. The gutter between two keys is sheet, and a press
/// on sheet is not a press on a key -- the platform's own soft keyboard
/// answers `None` there for the same reason.
pub fn key_at(
    x: usize,
    y: usize,
    shift: bool,
    symbols: bool,
    offset: usize,
) -> Option<KeyAction> {
    // WHY the dock clips the answer: a row scrolled past the dock's bottom
    // edge is not on the screen, so no press can land on it.
    if y < KB_Y0 || y >= KB_Y1 {
        return None;
    }
    let sheet = rows(symbols);
    for (index, row) in sheet.iter().enumerate() {
        let top = row_y(index, offset);
        for key in row.keys {
            if key_plate(key, top).contains(x, y) {
                return Some(shifted(key.action, shift));
            }
        }
    }
    None
}

/// The action a key performs once the shift state is applied.
///
/// WHY the case is applied here and not at every call site: a key that is
/// drawn in lower case and presses an upper-case letter is a keyboard that
/// lies about what it typed.
pub fn shifted(action: KeyAction, shift: bool) -> KeyAction {
    match action {
        KeyAction::Char(c) if !shift && c.is_ascii_alphabetic() => {
            KeyAction::Char(c.to_ascii_lowercase())
        }
        other => other,
    }
}

/// The character a key types and shows.
///
/// WHY this is a separate question from the action: a key that shows a
/// letter and types a different one is a key that lies about what it did,
/// and the two answers have to come from one place.
pub fn char_of(action: KeyAction) -> Option<u8> {
    match action {
        KeyAction::Char(c) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_letters_page_fits_the_dock_and_the_symbols_page_does_not() {
        // WHY this matters: a sheet that scrolls when it has nothing to
        // scroll is a control that moves for no reason, and a sheet that
        // cannot scroll when it has more content than the dock can show
        // hides the bottom of the symbols page.
        assert!(content_h(false) <= viewport_h());
        assert!(content_h(true) > viewport_h());
    }

    #[test]
    fn a_key_is_where_the_table_says_it_is() {
        let action = key_at(15, 994, false, false, 0);
        assert_eq!(action, Some(KeyAction::Char(b'q')));
        let action = key_at(15, 994, true, false, 0);
        assert_eq!(action, Some(KeyAction::Char(b'Q')));
        // Outside the sheet there is no key.
        assert_eq!(key_at(15, 900, false, false, 0), None);
        assert_eq!(key_at(15, 1400, false, false, 0), None);
    }

    #[test]
    fn the_sheet_scrolls_and_the_keys_move_with_it() {
        // WHY this matters: the sheet's bottom row is taller than the dock
        // that holds it, and a row that cannot be scrolled into view is a
        // character that cannot be typed.
        let last_row = SYMBOL_ROWS.len() - 1;
        let top = row_y(last_row, 0);
        assert!(
            top + KB_KEY_H > KB_Y1,
            "the last row fits without scrolling"
        );
        // At the top of the sheet the last row is cut off by the dock.
        assert!(top < KB_Y1, "the last row is not where the test expects it");
        // Scrolling brings the rest of the row into the dock.
        let offset = top + KB_KEY_H - KB_Y1;
        let visible_top = row_y(last_row, offset);
        assert!(visible_top >= KB_Y0);
        assert!(visible_top + KB_KEY_H <= KB_Y1);
        // The key under the finger is the one the sheet draws there.
        assert!(key_at(15, KB_Y1 - 1, false, true, 0).is_some());
        assert!(key_at(15, KB_Y1 - 1, false, true, offset).is_some());
    }

    #[test]
    fn the_shift_key_toggles_the_shift_state() {
        assert_eq!(
            key_at(15, 1158, false, false, 0),
            Some(KeyAction::Shift)
        );
    }

    #[test]
    fn the_symbols_key_switches_pages() {
        assert_eq!(
            key_at(15, 1240, false, false, 0),
            Some(KeyAction::Symbols)
        );
    }

    #[test]
    fn the_gutter_between_two_keys_answers_nothing() {
        // WHY this matters: the sheet draws 'Q' at 15..75 and 'W' at 85..145,
        // and the ten columns between them are sheet. A press there is a press
        // on the sheet, and the platform's own soft keyboard answers `None`
        // for it -- a keyboard that types a letter out of the gutter is a
        // keyboard whose answer and whose picture disagree.
        assert_eq!(key_at(74, 994, false, false, 0), Some(KeyAction::Char(b'q')));
        assert_eq!(key_at(75, 994, false, false, 0), None);
        assert_eq!(key_at(84, 994, false, false, 0), None);
        assert_eq!(key_at(85, 994, false, false, 0), Some(KeyAction::Char(b'w')));

        // The same between the rows: row 0 is painted 994..1066 and row 1
        // 1076..1148, and 'A' stands at x=46.
        assert_eq!(key_at(46, 1065, false, false, 0), Some(KeyAction::Char(b'q')));
        assert_eq!(key_at(46, 1066, false, false, 0), None);
        assert_eq!(key_at(46, 1075, false, false, 0), None);
        assert_eq!(key_at(46, 1076, false, false, 0), Some(KeyAction::Char(b'a')));
    }

    #[test]
    fn a_press_answers_the_key_whose_plate_it_is_on() {
        // WHY this matters: the painter fills the plate and the press answers
        // the plate, so every key on both pages has to answer on its own
        // pixels and nowhere else.
        for symbols in [false, true] {
            for (index, row) in rows(symbols).iter().enumerate() {
                let top = row_y(index, 0);
                for key in row.keys {
                    let plate = key_plate(key, top);
                    // A plate the dock has cut off is answered only where it is
                    // still on the screen.
                    let visible_y0 = plate.y.max(KB_Y0);
                    let visible_y1 = (plate.y + plate.h).min(KB_Y1);
                    if visible_y0 >= visible_y1 {
                        continue;
                    }
                    let cx = plate.x + plate.w / 2;
                    let cy = visible_y0 + (visible_y1 - visible_y0) / 2;
                    assert_eq!(
                        key_at(cx, cy, false, symbols, 0),
                        Some(shifted(key.action, false)),
                        "the plate at ({}, {}) has to answer its own key",
                        plate.x,
                        plate.y
                    );
                }
            }
        }
    }

    #[test]
    fn a_press_beside_a_plate_answers_nothing() {
        // WHY this matters: the sheet beside a key is sheet. The press used to
        // reach half a gutter into every gap, so the column to the left of
        // the first key and the row above the first key answered a letter
        // that was not drawn there.
        let q = key_plate(&LETTER_ROWS[0].keys[0], row_y(0, 0));
        assert_eq!(q.x, 15);
        assert_eq!(q.y, 994);
        assert_eq!(key_at(q.x - 1, q.y + q.h / 2, false, false, 0), None);
        assert_eq!(key_at(q.x + q.w / 2, q.y - 1, false, false, 0), None);
        // And the sheet below the last key of a row.
        let backspace = key_plate(&LETTER_ROWS[2].keys[LETTER_ROWS[2].keys.len() - 1], row_y(2, 0));
        assert_eq!(
            key_at(backspace.x + backspace.w, backspace.y + backspace.h / 2, false, false, 0),
            None
        );
    }

    #[test]
    fn backspace_has_generous_touch_target_above_48px() {
        // Row 2 Backspace key at x=620, w=85 (620..705), top=1158, h=72.
        // Touch target spans >= 48px in both dimensions and accepts edge touches.
        let plate = key_plate(&LETTER_ROWS[2].keys[LETTER_ROWS[2].keys.len() - 1], row_y(2, 0));
        assert!(plate.w >= 48 && plate.h >= 48);
        assert_eq!(key_at(620, 1158, false, false, 0), Some(KeyAction::Backspace));
        assert_eq!(key_at(660, 1190, false, false, 0), Some(KeyAction::Backspace));
        // The last painted row of the key answers, and the sheet past it does not.
        assert_eq!(key_at(704, 1229, false, false, 0), Some(KeyAction::Backspace));
        assert_eq!(key_at(705, 1229, false, false, 0), None);
        assert_eq!(key_at(710, 1200, false, false, 0), None);
    }
}
