// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The compose keyboard: the dock's geometry, the field it types into, and
//! what a key press does to that field.
//!
//! WHY this module exists: tapping the recipient field or the message body
//! gave the field focus and logged that it was focused, and nothing else
//! appeared. There was no keyboard on the screen, so there was no way to
//! type a message at all: the field took focus, nothing came up, and the
//! compose screen was a dead end.
//!
//! WHY the key table is the platform's: the keys, the hit testing and the
//! drawing come from the SDK's own soft-keyboard component, so the key under
//! the finger is the key that is drawn, and a third-party app that asks the
//! SDK for a keyboard gets the same one.
//!
//! WHY the wiring lives in the library and not in the binary: the app's
//! binary is built without its test harness, so a test that lived there
//! would never run. A keyboard that types the wrong character is exactly
//! the kind of defect only a test can catch.

use interos_std::design::components::SoftKeyboard;
use interos_std::design::primitives::Rect;

/// The key action the platform's keyboard emits.
pub use interos_std::design::components::KeyAction;

/// The width of the screen the keyboard is docked on.
pub const SCREEN_W: usize = 720;

/// The keyboard dock's height.
///
/// WHY this is the platform keyboard's own reference height: the key table
/// is laid out in a 720x430 reference box, and a dock of any other height
/// would scale the keys, so a key drawn 60 wide could be pressed somewhere
/// else.
pub const KB_HEIGHT: usize = 430;

/// The keyboard dock's top edge.
///
/// WHY the dock stops short of the bottom of the screen: the system's home
/// gesture lives in the last rows of the display, and a keyboard drawn over
/// it would eat the gesture that leaves the app.
pub const KB_Y0: usize = 980;

/// The keyboard dock's bottom edge.
pub const KB_Y1: usize = KB_Y0 + KB_HEIGHT;

/// The height of the recipient ("To:") band.
pub const RECIPIENT_BAND_H: usize = 46;

/// The height of the compose band.
pub const COMPOSE_BAND_H: usize = 84;

/// The height of the recipient band and the compose band together.
pub const BANDS_H: usize = RECIPIENT_BAND_H + COMPOSE_BAND_H;

/// Where the two bands sit when the keyboard is down.
pub const BANDS_TOP_IDLE: usize = 1284;

/// The row the two bands end on when the keyboard is down.
///
/// WHY this is the edge that is fixed and not the top: the composer's ink stops
/// here and the home gesture claims every row from here down (`hit_from` in the
/// application). A band that grew its field therefore buys its rows from the top
/// -- out of the scrolling list -- and never from the gesture.
pub const BANDS_BOTTOM_IDLE: usize = BANDS_TOP_IDLE + BANDS_H;

/// The rows the recipient band is given beyond its anchor, for the text size the
/// application is drawing at.
///
/// WHY a cell and not an argument threaded through every layout number: the
/// application publishes the person's rung once, before the surface exists, and
/// sets this in the same call. Every band edge below then reads one integer
/// instead of asking the settings store, which is what keeps a settings query out
/// of the paint loop. Zero at the ladder, so the dock and both band places stay
/// exactly where the platform's own keyboard reference box puts them.
static BAND_EXTRA_ROWS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Records the rows the recipient band has to give its field at the published
/// level. Called once, by the application, beside `DynamicType::publish`.
pub fn set_band_extra_rows(rows: usize) {
    BAND_EXTRA_ROWS.store(rows, core::sync::atomic::Ordering::Relaxed);
}

/// The rows the recipient band is carrying beyond its anchor.
#[inline(always)]
pub fn band_extra_rows() -> usize {
    BAND_EXTRA_ROWS.load(core::sync::atomic::Ordering::Relaxed)
}

/// The recipient band's height at the level the application is drawing at.
#[inline(always)]
pub fn recipient_band_h() -> usize {
    RECIPIENT_BAND_H + band_extra_rows()
}

/// The height of both bands together at the published level.
#[inline(always)]
pub fn bands_h() -> usize {
    BANDS_H + band_extra_rows()
}

/// Where the two bands sit when the keyboard is up: directly above it.
///
/// WHY the bands sit on the keyboard's edge and not a row above it: a band
/// with a gap under it leaves a strip of screen that belongs to neither, and
/// the composer looks like it is floating.
pub const BANDS_TOP_KEYBOARD: usize = KB_Y0 - BANDS_H;

/// Where the two bands sit in the layout the app is drawing right now.
///
/// WHY the bands move: the keyboard is docked at the bottom of the screen,
/// and a compose control the keyboard covers is a control the user cannot
/// reach. The bands sit directly above the keyboard while it is up, and
/// return to the bottom of the screen when it is dismissed.
///
/// WHY the height is asked and the anchor subtracted from the edges: the bands
/// keep their bottom edge at every text size (see [`BANDS_BOTTOM_IDLE`]), so a
/// taller field lifts their top by exactly the rows it needed.
#[inline(always)]
pub fn bands_top(keyboard: &Keyboard) -> usize {
    if keyboard.is_visible() {
        KB_Y0 - bands_h()
    } else {
        BANDS_BOTTOM_IDLE - bands_h()
    }
}

/// The screen region a change to the keyboard has to repaint.
///
/// WHY the region holds both band positions and not the one the bands are
/// in now: the bands move when the keyboard comes up and when it goes
/// down, and a redraw that covered only the position they are in now would
/// leave the pixels of the other position on the screen.
pub fn dirty_region(keyboard: &Keyboard) -> (usize, usize) {
    let here = bands_top(keyboard);
    // The two places the bands can be, each lifted by the rows the published
    // level has given the field.
    let idle = BANDS_TOP_IDLE - band_extra_rows();
    let raised = BANDS_TOP_KEYBOARD - band_extra_rows();
    let other = if keyboard.is_visible() { idle } else { raised };
    let top = here.min(other);
    let bottom = (here + bands_h())
        .max(other + bands_h())
        .max(idle + bands_h())
        .max(raised + bands_h())
        .max(KB_Y1);
    (top, bottom)
}

/// Which field the keyboard types into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The thread list's search field.
    Search,
    /// The recipient ("To:") field.
    Recipient,
    /// The message body.
    Body,
}

/// One bounded text field, as the keyboard sees it.
///
/// WHY this is a borrowed view and not the field itself: the app owns the
/// bytes, and the keyboard borrows them for the length of one key press.
pub struct Editor<'a> {
    /// The field's bytes.
    pub buf: &'a mut [u8],
    /// How many of the bytes are in use.
    pub len: &'a mut usize,
    /// Whether the whole field is selected.
    pub select_all: &'a mut bool,
}

/// The three text fields the keyboard types into.
///
/// WHY they are one value and not three arguments: a key press has to go
/// to exactly one of them, and a caller that passed three buffers had to
/// pick one by hand at every call site.
pub struct Editors<'a> {
    /// The thread list's search field.
    pub search: Editor<'a>,
    /// The recipient ("To:") field.
    pub recipient: Editor<'a>,
    /// The message body.
    pub body: Editor<'a>,
}

/// The keyboard's own state.
///
/// WHY the focused field lives here and not in three booleans on the app:
/// the keyboard and the focus are the same fact. A field that is focused
/// shows the keyboard, and a keyboard with no focused field has nothing to
/// type into, and two places that have to agree are two places that can
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyboard {
    /// The field the keyboard types into, if any.
    field: Option<Field>,
    /// Whether the shift state is on.
    shift: bool,
    /// The key that was pressed last, so the renderer can draw it pressed.
    pressed: Option<KeyAction>,
}

impl Keyboard {
    /// A keyboard that is not on the screen.
    pub const fn new() -> Self {
        Self {
            field: None,
            shift: false,
            pressed: None,
        }
    }

    /// Whether the keyboard is on the screen.
    pub const fn is_visible(&self) -> bool {
        self.field.is_some()
    }

    /// The field the keyboard types into.
    pub const fn field(&self) -> Option<Field> {
        self.field
    }

    /// Whether the keyboard types into `field`.
    ///
    /// WHY the arms are spelled out: `==` on the field is not a const
    /// operation, and this predicate is asked on every pixel of the screen.
    pub const fn is_focused(&self, field: Field) -> bool {
        match (self.field, field) {
            (Some(Field::Search), Field::Search) => true,
            (Some(Field::Recipient), Field::Recipient) => true,
            (Some(Field::Body), Field::Body) => true,
            _ => false,
        }
    }

    /// Whether the shift state is on.
    pub const fn shift(&self) -> bool {
        self.shift
    }

    /// The key that was pressed last.
    pub const fn pressed(&self) -> Option<KeyAction> {
        self.pressed
    }

    /// Puts the keyboard in front of `field`.
    pub fn focus(&mut self, field: Field) {
        self.field = Some(field);
    }

    /// Takes the keyboard off the screen.
    pub fn dismiss(&mut self) {
        self.field = None;
        self.pressed = None;
    }

    /// Turns the shift state on or off.
    pub fn toggle_shift(&mut self) {
        self.shift = !self.shift;
    }

    /// Records the key that was pressed, so the renderer can draw it pressed.
    pub fn set_pressed(&mut self, action: Option<KeyAction>) {
        self.pressed = action;
    }

    /// The platform widget the renderer draws and the tap handler hits.
    pub fn widget(&self) -> SoftKeyboard {
        SoftKeyboard::new(Rect::new(0, KB_Y0, SCREEN_W, KB_HEIGHT))
            .with_shift(self.shift)
            .with_pressed(self.pressed)
    }

    /// The key under the finger, if there is one.
    ///
    /// WHY this is the platform's hit testing and not a second table: a key
    /// that is drawn in one place and pressed in another is a keyboard that
    /// lies about what it typed.
    pub fn action_at(&self, x: usize, y: usize) -> Option<KeyAction> {
        self.widget().hit_test(x, y)
    }

    /// Applies a key press to the field the keyboard is focused on.
    ///
    /// WHY the routing lives here and not in the app: the focused field is
    /// the keyboard's own fact, and a caller that picked the editor by
    /// hand could type into a field the caret is not in.
    ///
    /// Returns whether the text changed.
    pub fn apply_to(&mut self, action: KeyAction, editors: &mut Editors<'_>) -> bool {
        let field = match self.field {
            Some(field) => field,
            None => return false,
        };
        let editor = match field {
            Field::Search => &mut editors.search,
            Field::Recipient => &mut editors.recipient,
            Field::Body => &mut editors.body,
        };
        self.apply(action, editor.buf, editor.len, editor.select_all)
    }

    /// Applies a key press to a bounded text buffer.
    ///
    /// WHY the buffer comes in as an argument: the app holds three buffers
    /// of three different capacities, and a keyboard that owned one of them
    /// would have to be told which one it was typing into.
    ///
    /// WHY this is not the entry point: a caller that reached for it would
    /// have to choose the buffer itself, and the field the caret is in is
    /// the keyboard's own fact. `apply_to` is the way in.
    ///
    /// Returns whether the text changed.
    fn apply(
        &mut self,
        action: KeyAction,
        buf: &mut [u8],
        len: &mut usize,
        select_all: &mut bool,
    ) -> bool {
        match action {
            KeyAction::Char(c) => {
                // WHY the selection is replaced first: the field is focused
                // with everything selected so that the first character the
                // user types replaces the old text, and a character typed
                // into a selection that is still there would be inserted
                // into text the user meant to replace.
                if *select_all {
                    *len = 0;
                    *select_all = false;
                }
                if *len >= buf.len() {
                    return false;
                }
                buf[*len] = c;
                *len += 1;
                true
            }
            KeyAction::Backspace => {
                if *select_all {
                    *len = 0;
                    *select_all = false;
                    return true;
                }
                if *len == 0 {
                    return false;
                }
                *len -= 1;
                true
            }
            _ => false,
        }
    }
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The centre of the key that types `c`, in screen coordinates.
    fn key_point(c: u8) -> (usize, usize) {
        for key in SoftKeyboard::keys() {
            if key.unshifted == KeyAction::Char(c) {
                return (key.x + key.w / 2, KB_Y0 + key.y + key.h / 2);
            }
        }
        panic!("the key table has no key for that character");
    }

    #[test]
    fn the_keyboard_is_shown_only_while_a_field_is_focused() {
        let mut kb = Keyboard::new();
        assert!(!kb.is_visible());
        assert!(kb.field().is_none());

        kb.focus(Field::Body);
        assert!(kb.is_visible());
        assert!(kb.is_focused(Field::Body));
        assert!(!kb.is_focused(Field::Search));
        assert!(!kb.is_focused(Field::Recipient));

        kb.dismiss();
        assert!(!kb.is_visible());
        assert!(kb.field().is_none());
    }

    #[test]
    fn a_tap_on_the_key_for_a_types_an_a_into_the_focused_field() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);

        let (x, y) = key_point(b'a');
        let action = kb.action_at(x, y).expect("the tap lands on a key");
        assert_eq!(action, KeyAction::Char(b'a'));

        let mut buf = [0u8; 16];
        let mut len = 0usize;
        let mut select_all = false;
        assert!(kb.apply(action, &mut buf, &mut len, &mut select_all));
        assert_eq!(&buf[..len], b"a");
    }

    #[test]
    fn a_tap_on_the_key_for_a_types_an_uppercase_a_while_shift_is_on() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);
        kb.toggle_shift();
        assert!(kb.shift());

        let (x, y) = key_point(b'a');
        let action = kb.action_at(x, y).expect("the tap lands on a key");
        assert_eq!(action, KeyAction::Char(b'A'));

        let mut buf = [0u8; 16];
        let mut len = 0usize;
        let mut select_all = false;
        assert!(kb.apply(action, &mut buf, &mut len, &mut select_all));
        assert_eq!(&buf[..len], b"A");
    }

    #[test]
    fn backspace_removes_the_last_character() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);

        let mut buf = [0u8; 16];
        let mut len = 0usize;
        let mut select_all = false;
        kb.apply(KeyAction::Char(b'h'), &mut buf, &mut len, &mut select_all);
        kb.apply(KeyAction::Char(b'i'), &mut buf, &mut len, &mut select_all);
        assert_eq!(&buf[..len], b"hi");

        assert!(kb.apply(
            KeyAction::Backspace,
            &mut buf,
            &mut len,
            &mut select_all
        ));
        assert_eq!(&buf[..len], b"h");
    }

    #[test]
    fn backspace_on_an_empty_field_changes_nothing() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);
        let mut buf = [0u8; 16];
        let mut len = 0usize;
        let mut select_all = false;
        assert!(!kb.apply(
            KeyAction::Backspace,
            &mut buf,
            &mut len,
            &mut select_all
        ));
        assert_eq!(len, 0);
    }

    #[test]
    fn the_first_character_replaces_a_selected_field() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);
        let mut buf = [0u8; 16];
        buf[..2].copy_from_slice(b"hi");
        let mut len = 2usize;
        let mut select_all = true;
        assert!(kb.apply(
            KeyAction::Char(b'x'),
            &mut buf,
            &mut len,
            &mut select_all
        ));
        assert_eq!(&buf[..len], b"x");
        assert!(!select_all);
    }

    #[test]
    fn a_full_field_refuses_another_character() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);
        let mut buf = [0u8; 2];
        let mut len = 2usize;
        let mut select_all = false;
        assert!(!kb.apply(
            KeyAction::Char(b'x'),
            &mut buf,
            &mut len,
            &mut select_all
        ));
        assert_eq!(len, 2);
    }

    #[test]
    fn the_hide_key_and_the_enter_key_are_reachable() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);

        // The dock's last row carries the hide key and the enter key.
        let hide = kb.action_at(40, KB_Y0 + 360).expect("the hide key is there");
        assert_eq!(hide, KeyAction::Hide);
        let enter = kb.action_at(640, KB_Y0 + 360).expect("the enter key is there");
        assert_eq!(enter, KeyAction::Enter);
    }

    #[test]
    fn a_tap_outside_the_dock_finds_no_key() {
        let kb = Keyboard::new();
        assert!(kb.action_at(360, KB_Y0 - 10).is_none());
        assert!(kb.action_at(360, KB_Y1).is_none());
    }

    #[test]
    fn the_keyboard_does_not_cover_the_compose_controls() {
        let mut kb = Keyboard::new();

        // The keyboard is down: the bands sit at the bottom of the screen.
        assert_eq!(bands_top(&kb), BANDS_TOP_IDLE);
        assert_eq!(BANDS_TOP_IDLE + BANDS_H, 1414);

        // The keyboard is up: the two bands move up and sit on the dock's
        // top edge, so the send button and the recipient picker stay
        // above the keyboard and stay reachable.
        kb.focus(Field::Body);
        assert_eq!(bands_top(&kb), BANDS_TOP_KEYBOARD);
        assert_eq!(BANDS_TOP_KEYBOARD + BANDS_H, KB_Y0);
        // The recipient band, which holds the recipient picker, is above
        // the keyboard's top edge too.
        assert!(BANDS_TOP_KEYBOARD + RECIPIENT_BAND_H <= KB_Y0);
    }

    #[test]
    fn the_bands_move_when_the_keyboard_comes_up_and_back_when_it_goes_down() {
        let mut kb = Keyboard::new();
        let idle = bands_top(&kb);

        kb.focus(Field::Body);
        let raised = bands_top(&kb);
        assert_ne!(raised, idle, "the bands did not move for the keyboard");
        assert_eq!(raised + BANDS_H, KB_Y0, "the bands do not sit on the dock");

        kb.dismiss();
        assert_eq!(bands_top(&kb), idle, "the bands did not come back");
    }

    #[test]
    fn the_dirty_region_covers_the_dock_and_both_positions_of_the_bands() {
        let mut kb = Keyboard::new();

        // The keyboard is down: the region still holds the place the bands
        // will move to, because a change that raises the keyboard leaves
        // the old pixels of the bands behind.
        let (y0, y1) = dirty_region(&kb);
        assert!(y0 <= BANDS_TOP_KEYBOARD);
        assert!(y1 >= BANDS_TOP_IDLE + BANDS_H);
        assert!(y0 <= BANDS_TOP_IDLE && y1 >= BANDS_TOP_IDLE + BANDS_H);

        // The keyboard is up: the region covers the bands where they are
        // now and the dock they sit on.
        kb.focus(Field::Body);
        let (y0, y1) = dirty_region(&kb);
        assert!(y0 <= BANDS_TOP_KEYBOARD);
        assert!(y1 >= KB_Y1, "the dock is not in the dirty region");
        assert!(y0 <= bands_top(&kb) && y1 >= bands_top(&kb) + BANDS_H);
    }

    #[test]
    fn a_key_press_goes_to_the_focused_field_and_to_no_other() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Recipient);

        let mut search_buf = [0u8; 8];
        let mut search_len = 0usize;
        let mut search_sel = false;
        let mut recip_buf = [0u8; 8];
        let mut recip_len = 0usize;
        let mut recip_sel = false;
        let mut body_buf = [0u8; 8];
        let mut body_len = 0usize;
        let mut body_sel = false;

        {
            let mut editors = Editors {
                search: Editor {
                    buf: &mut search_buf,
                    len: &mut search_len,
                    select_all: &mut search_sel,
                },
                recipient: Editor {
                    buf: &mut recip_buf,
                    len: &mut recip_len,
                    select_all: &mut recip_sel,
                },
                body: Editor {
                    buf: &mut body_buf,
                    len: &mut body_len,
                    select_all: &mut body_sel,
                },
            };
            assert!(kb.apply_to(KeyAction::Char(b'x'), &mut editors));
        }

        assert_eq!(&recip_buf[..recip_len], b"x");
        assert_eq!(search_len, 0);
        assert_eq!(body_len, 0);
    }

    #[test]
    fn a_key_press_reaches_the_body_when_the_body_is_focused() {
        let mut kb = Keyboard::new();
        kb.focus(Field::Body);

        let mut search_buf = [0u8; 8];
        let mut search_len = 0usize;
        let mut search_sel = false;
        let mut recip_buf = [0u8; 8];
        let mut recip_len = 0usize;
        let mut recip_sel = false;
        let mut body_buf = [0u8; 8];
        let mut body_len = 0usize;
        let mut body_sel = false;

        {
            let mut editors = Editors {
                search: Editor {
                    buf: &mut search_buf,
                    len: &mut search_len,
                    select_all: &mut search_sel,
                },
                recipient: Editor {
                    buf: &mut recip_buf,
                    len: &mut recip_len,
                    select_all: &mut recip_sel,
                },
                body: Editor {
                    buf: &mut body_buf,
                    len: &mut body_len,
                    select_all: &mut body_sel,
                },
            };
            assert!(kb.apply_to(KeyAction::Char(b'h'), &mut editors));
            assert!(kb.apply_to(KeyAction::Char(b'i'), &mut editors));
            assert!(kb.apply_to(KeyAction::Backspace, &mut editors));
        }

        assert_eq!(&body_buf[..body_len], b"h");
        assert_eq!(search_len, 0);
        assert_eq!(recip_len, 0);
    }

    #[test]
    fn a_key_press_with_no_field_focused_changes_nothing() {
        let mut kb = Keyboard::new();

        let mut search_buf = [0u8; 8];
        let mut search_len = 0usize;
        let mut search_sel = false;
        let mut recip_buf = [0u8; 8];
        let mut recip_len = 0usize;
        let mut recip_sel = false;
        let mut body_buf = [0u8; 8];
        let mut body_len = 0usize;
        let mut body_sel = false;

        {
            let mut editors = Editors {
                search: Editor {
                    buf: &mut search_buf,
                    len: &mut search_len,
                    select_all: &mut search_sel,
                },
                recipient: Editor {
                    buf: &mut recip_buf,
                    len: &mut recip_len,
                    select_all: &mut recip_sel,
                },
                body: Editor {
                    buf: &mut body_buf,
                    len: &mut body_len,
                    select_all: &mut body_sel,
                },
            };
            assert!(!kb.apply_to(KeyAction::Char(b'x'), &mut editors));
        }

        assert_eq!(search_len, 0);
        assert_eq!(recip_len, 0);
        assert_eq!(body_len, 0);
    }
}
