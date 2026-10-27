//! The browser's bounded document layout engine.
//!
//! The reader path flattens a page into styled lines. This module keeps the
//! element tree and turns it into positioned boxes and text runs, so the
//! browser can lay out headings, paragraphs, lists, tables, images and form
//! controls in a real column instead of a single stream of lines.
//!
//! The engine is zero-allocation: the DOM, the computed styles and the
//! display list all live in fixed-capacity arrays.

pub mod dom;
pub mod flow;
pub mod style;

pub use dom::{Dom, NodeId, NO_LINK, NO_NODE, StrRef};
pub use flow::{
    build, overflow_note, FieldItem, FormInfo, FormMethod, ImageItem, LinkBox, RectItem, RectKind,
    TextLayout, TextRun, MAX_FIELDS, MAX_IMAGES, MAX_LINK_BOXES, MAX_RECTS, MAX_TEXT_RUNS,
    OVERFLOW_BUDGETS,
};
pub use style::{BoxStyle, Display, FieldKind, InputType};
