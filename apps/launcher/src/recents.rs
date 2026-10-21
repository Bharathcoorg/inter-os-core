//! The task switcher's card model.
//!
//! WHY a model and not a painter: the switcher has to answer three questions
//! about a running task -- what it is called, what it is showing, and what a
//! gesture on its card means -- and the answers have to be the same in the
//! frame, in the hit test and in the test suite. So the geometry, the card's
//! contents and the gesture thresholds live here, and the painter and the
//! touch handler both read them.
//!
//! WHY the preview is part of the card: a card that cannot show what the app
//! is showing is a picture of nothing. The platform can now hand over a
//! running app's real pixels to a client that holds the capture capability,
//! and when it cannot, the card says why rather than drawing a picture of
//! what an app of that kind might look like.

use crate::layout::Rect;

/// What the card knows about the app's picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// The platform returned the app's current pixels, scaled into the well.
    Live {
        /// The width of the picture, in pixels.
        width: u16,
        /// The height of the picture, in pixels.
        height: u16,
    },
    /// The app has closed, and these are the last pixels it drew.
    ///
    /// WHY this is a state of its own and not `Live`: an app in this platform
    /// exits when the person leaves it, so the picture a card holds is the
    /// app's last frame rather than its current one. Calling that a live
    /// preview would be a claim the card cannot back.
    LastFrame {
        /// The width of the picture, in pixels.
        width: u16,
        /// The height of the picture, in pixels.
        height: u16,
    },
    /// The platform could not produce a picture; the reason says why.
    ///
    /// WHY the reason is part of the value: "no preview" and "the shell is
    /// not allowed to read this app's screen" are different facts, and a card
    /// that said only "no preview" would hide a permission decision behind a
    /// shrug.
    Unavailable(&'static str),
}

impl Preview {
    /// Whether the card has real pixels to draw.
    pub fn is_live(&self) -> bool {
        matches!(self, Preview::Live { .. })
    }

    /// Whether the card holds a picture of any kind.
    pub fn has_picture(&self) -> bool {
        matches!(self, Preview::Live { .. } | Preview::LastFrame { .. })
    }

    /// The reason the preview is missing, if it is.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Preview::Live { .. } | Preview::LastFrame { .. } => None,
            Preview::Unavailable(reason) => Some(reason),
        }
    }
}

/// One running task's card.
#[derive(Debug, Clone, Copy)]
pub struct Card {
    /// The package the task is running.
    pub package_id: [u8; 40],
    /// The length of the package id.
    pub package_len: u8,
    /// The name the card shows.
    pub label: [u8; 24],
    /// The length of the label.
    pub label_len: u8,
    /// The task the card is for.
    pub task_id: u64,
    /// Whether this is the task in the foreground.
    pub focused: bool,
    /// What the card can show of the app.
    pub preview: Preview,
}

impl Card {
    /// An empty card.
    pub const EMPTY: Card = Card {
        package_id: [0; 40],
        package_len: 0,
        label: [0; 24],
        label_len: 0,
        task_id: 0,
        focused: false,
        preview: Preview::Unavailable("no task"),
    };

    /// The package the task is running.
    pub fn package(&self) -> &[u8] {
        &self.package_id[..self.package_len as usize]
    }

    /// The name the card shows.
    pub fn label(&self) -> &[u8] {
        &self.label[..self.label_len as usize]
    }
}

/// How many cards the stack shows at once.
pub const MAX_CARDS: usize = 3;

/// The card's left edge.
pub const CARD_X: usize = 32;
/// The card's width.
pub const CARD_W: usize = 656;
/// The card's height.
pub const CARD_H: usize = 340;
/// The gap between two cards.
pub const CARD_GAP: usize = 12;
/// The top of the first card.
pub const CARDS_Y0: usize = 176;

/// The live preview's width.
pub const PREVIEW_W: usize = 150;
/// The live preview's height.
///
/// WHY the preview keeps the source's shape: a task switcher shows the app's
/// screen, and a screen that has been squashed into a different shape is a
/// picture of something else. The guest surface is 720 x 1440, so the well is
/// half that in each direction.
pub const PREVIEW_H: usize = 300;

/// The card at `index`.
pub fn card_rect(index: usize) -> Rect {
    Rect::new(
        CARD_X,
        CARDS_Y0 + index * (CARD_H + CARD_GAP),
        CARD_W,
        CARD_H,
    )
}

/// The live preview well of the card at `index`.
pub fn preview_rect(index: usize) -> Rect {
    let card = card_rect(index);
    Rect::new(card.x + 20, card.y + 20, PREVIEW_W, PREVIEW_H)
}

/// The close control of the card at `index`.
///
/// WHY the close control is 44 wide: that is the platform's minimum touch
/// target, and a control that is easier to miss than to hit is not a
/// control.
pub fn close_rect(index: usize) -> Rect {
    let card = card_rect(index);
    Rect::new(card.x + CARD_W - 64, card.y + 16, 44, 44)
}

/// The app icon of the card at `index`.
pub fn icon_rect(index: usize) -> Rect {
    let card = card_rect(index);
    Rect::new(card.x + 196, card.y + 24, 56, 56)
}

/// The card at `index` whose area holds (`x`, `y`).
pub fn card_at(x: usize, y: usize, count: usize) -> Option<usize> {
    for index in 0..count.min(MAX_CARDS) {
        if card_rect(index).contains(x, y) {
            return Some(index);
        }
    }
    None
}

/// The close control at (`x`, `y`).
pub fn close_at(x: usize, y: usize, count: usize) -> Option<usize> {
    for index in 0..count.min(MAX_CARDS) {
        if close_rect(index).contains(x, y) {
            return Some(index);
        }
    }
    None
}

/// What a vertical gesture on the stack means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swipe {
    /// The gesture was too small to mean anything.
    None,
    /// The gesture dismissed the card it started on.
    Dismiss,
    /// The gesture went home.
    Home,
}

/// The distance a gesture has to travel to dismiss a card.
pub const DISMISS_DISTANCE: i32 = 96;
/// The distance a gesture has to travel to go home.
pub const HOME_DISTANCE: i32 = 96;

/// Reads a vertical gesture.
///
/// WHY up dismisses and down goes home: the card stack scrolls down to the
/// older tasks, so a downward gesture is a scroll and must not also close
/// anything, while an upward gesture is not a scroll at all and is the
/// gesture people already use to throw a card away.
pub fn classify_swipe(dy: i32) -> Swipe {
    if dy <= -DISMISS_DISTANCE {
        Swipe::Dismiss
    } else if dy >= HOME_DISTANCE {
        Swipe::Home
    } else {
        Swipe::None
    }
}
