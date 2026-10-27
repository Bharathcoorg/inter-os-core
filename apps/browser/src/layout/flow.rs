//! Block and inline layout for the browser view.
//!
//! The engine turns a styled DOM into a display list: filled and outlined
//! rectangles, text runs, image slots and form fields. The renderer paints
//! that list; hit testing reads the link boxes and field rectangles from
//! it.
//!
//! The model is a deliberately small block layout: every block owns the
//! column it is given, inline content is wrapped into lines, lists get a
//! marker, tables divide their width equally between columns, and form
//! controls occupy a box in the line. Floats, flexbox, grid, absolute
//! positioning and margin collapsing are not implemented and are not
//! approximated.

use crate::css;
use crate::std::design::font;
use super::dom::{Dom, NodeId, StrRef, NO_LINK};
use super::style::{self, BoxStyle, Display, FieldKind, InputType};

/// The most text runs one page's display list holds.
pub const MAX_TEXT_RUNS: usize = 4096;
/// The most rectangles (backgrounds, borders, rules) one list holds.
pub const MAX_RECTS: usize = 768;
/// The most images the layout places. This matches the image arena the
/// browser fetches into.
pub const MAX_IMAGES: usize = 4;
/// The most form controls one page holds.
pub const MAX_FIELDS: usize = 48;
/// The most link boxes one page holds.
pub const MAX_LINK_BOXES: usize = 128;
/// How many inline segments are buffered before they are laid into lines.
///
/// WHY a buffer that is emptied as it fills: the list is one per page, not one
/// per block, so this bounds how much of a paragraph is held at a time and not
/// how long a paragraph can be. It used to be one list per block level, which
/// made the same number a bound on every paragraph on the page and a charge
/// against the app's stack at every level of nesting.
const MAX_SEGMENTS: usize = 512;
/// The deepest block nesting the layout descends.
///
/// This is a stack bound, not a document bound. Every block level costs a
/// frame holding a computed style (240 bytes) and a few cursors, so the app's
/// 256 KiB user stack carries this depth with room to spare now that the
/// segment list is one buffer for the page rather than one per level. A
/// deeper block is laid out through its nearest tracked ancestor and the
/// display list reports the block depth it ran out of.
const MAX_LAYOUT_DEPTH: usize = 32;
/// The width a form control takes when neither markup nor style names one.
const DEFAULT_FIELD_W: usize = 260;
/// The side of a checkbox or radio button.
const TOGGLE_FIELD_W: usize = 24;
/// The bullet a list item shows when its parent is not an ordered list.
const BULLET: &[u8] = "•".as_bytes();

/// Which of the layout's own budgets was reached.
///
/// WHY per budget and not one flag: `truncated` is the honest thing to show a
/// reader and the useless thing to show whoever has to fix the page. A page
/// that ran out of text runs and a page nested deeper than the layout descends
/// both say `true` today, and they need different budgets raised.
///
/// The low bits belong to the document tree and are ORed in from
/// [`super::dom`], so one value names every budget a page can hit.
pub const OVERFLOW_SEGMENTS: u32 = 1 << 8;
pub const OVERFLOW_BLOCK_DEPTH: u32 = 1 << 9;
pub const OVERFLOW_RUNS: u32 = 1 << 10;
pub const OVERFLOW_RECTS: u32 = 1 << 11;
pub const OVERFLOW_IMAGES: u32 = 1 << 12;
pub const OVERFLOW_FIELDS: u32 = 1 << 13;
pub const OVERFLOW_LINKS: u32 = 1 << 14;
pub const OVERFLOW_COORDS: u32 = 1 << 15;

/// Every budget a page can hit, with what ran out, ranked worst first.
///
/// WHY a table and not an `if` chain: the sentence drawn on the page and the
/// lines written to the serial log have to name the same budgets, and two
/// spellings of one list drift apart. The order is the ranking -- the first
/// entry whose bit is set is the one a reader hears about, because missing
/// content matters more than missing decoration.
pub const OVERFLOW_BUDGETS: &[(u32, &str)] = &[
    (
        super::dom::OVERFLOW_NODES,
        "The document is larger than this browser holds; the rest of it was not read.",
    ),
    (
        super::dom::OVERFLOW_TEXT,
        "The document has more words than this browser holds; some of them are missing.",
    ),
    (
        super::dom::OVERFLOW_DEPTH,
        "The page nests deeper than this browser reads; some elements are missing.",
    ),
    (
        OVERFLOW_BLOCK_DEPTH,
        "The page nests deeper than this browser lays out; some blocks are missing.",
    ),
    (
        OVERFLOW_SEGMENTS,
        "A paragraph here is longer than this browser measures; part of it is missing.",
    ),
    (
        OVERFLOW_RUNS,
        "The page has more text than this browser holds; some of it is missing.",
    ),
    (
        OVERFLOW_COORDS,
        "The page is longer than this browser can address; the rest is off the end.",
    ),
    (
        OVERFLOW_IMAGES,
        "The page has more images than this browser fetches; some are missing.",
    ),
    (
        OVERFLOW_LINKS,
        "The page has more links than this browser tracks; some cannot be tapped.",
    ),
    (
        OVERFLOW_FIELDS,
        "The page has more form controls than this browser holds; some are missing.",
    ),
    (
        super::dom::OVERFLOW_ATTRS,
        "The document has more attributes than this browser holds; some tags are unread.",
    ),
    (
        super::dom::OVERFLOW_CSS,
        "The document has more styling than this browser holds; some of it is unread.",
    ),
    (
        OVERFLOW_RECTS,
        "The page has more panels and borders than this browser paints.",
    ),
];

/// What to tell a reader about a page that did not all fit.
///
/// WHY one sentence: the note is drawn on the page, in a single line, and a
/// list of bit flags is not a sentence. When several budgets were hit the
/// reader hears about the worst one; the serial log carries every one.
pub fn overflow_note(overflow: u32) -> &'static str {
    match OVERFLOW_BUDGETS.iter().find(|(bit, _)| overflow & *bit != 0) {
        Some((_, reason)) => reason,
        None => "The page reached a layout capacity; it is shown in part.",
    }
}

/// What a rectangle means to the renderer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RectKind {
    Fill,
    Outline,
    Rule,
}

/// One painted rectangle.
#[derive(Clone, Copy, Debug)]
pub struct RectItem {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    pub color: u32,
    pub kind: RectKind,
}

impl RectItem {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        color: 0,
        kind: RectKind::Fill,
    };
}

/// One run of text, positioned in the document column.
#[derive(Clone, Copy, Debug)]
pub struct TextRun {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    pub scale: u8,
    pub bold: bool,
    pub color: u32,
    pub has_color: bool,
    /// The bytes, when they come from the document.
    pub text: StrRef,
    /// A short literal (a list number or bullet), used instead of `text`.
    pub literal: [u8; 4],
    pub literal_len: u8,
    pub link: u8,
}

impl TextRun {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        scale: 1,
        bold: false,
        color: 0,
        has_color: false,
        text: StrRef::EMPTY,
        literal: [0; 4],
        literal_len: 0,
        link: NO_LINK,
    };

    pub fn bytes<'a>(&'a self, dom: &'a Dom) -> &'a [u8] {
        if self.literal_len > 0 {
            &self.literal[..self.literal_len as usize]
        } else {
            self.text.get(dom)
        }
    }
}

/// One image slot in the display list.
#[derive(Clone, Copy, Debug)]
pub struct ImageItem {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    /// The slot in the browser's placed-image array, or 255 for none.
    pub slot: u8,
    pub src: StrRef,
    pub alt: StrRef,
    pub link: u8,
}

impl ImageItem {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        slot: 255,
        src: StrRef::EMPTY,
        alt: StrRef::EMPTY,
        link: NO_LINK,
    };
}

/// One form control. The fields are what a submission would send.
#[derive(Clone, Copy, Debug)]
pub struct FieldItem {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    /// The element the control came from, so its form and options can be
    /// found in the tree when it is used.
    pub node: NodeId,
    pub kind: FieldKind,
    pub input: InputType,
    pub name: StrRef,
    pub value: StrRef,
    pub placeholder: StrRef,
    pub checked: bool,
}

impl FieldItem {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        node: super::dom::NO_NODE,
        kind: FieldKind::None,
        input: InputType::Text,
        name: StrRef::EMPTY,
        value: StrRef::EMPTY,
        placeholder: StrRef::EMPTY,
        checked: false,
    };
}

/// One tappable link area.
#[derive(Clone, Copy, Debug)]
pub struct LinkBox {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
    pub link: u8,
}

impl LinkBox {
    pub const EMPTY: Self = Self {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        link: NO_LINK,
    };

    pub fn contains(&self, x: usize, y: usize) -> bool {
        let (x, y) = (x as i32, y as i32);
        x >= self.x as i32
            && y >= self.y as i32
            && x < self.x as i32 + self.w as i32
            && y < self.y as i32 + self.h as i32
    }
}

/// How a form sends its fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormMethod {
    Get,
    Post,
}

/// The first form on the page, if any.
#[derive(Clone, Copy, Debug)]
pub struct FormInfo {
    pub action: StrRef,
    pub method: FormMethod,
    pub present: bool,
}

impl FormInfo {
    pub const EMPTY: Self = Self {
        action: StrRef::EMPTY,
        method: FormMethod::Get,
        present: false,
    };
}

/// The complete display list for one page.
pub struct TextLayout {
    pub runs: [TextRun; MAX_TEXT_RUNS],
    pub run_count: usize,
    pub rects: [RectItem; MAX_RECTS],
    pub rect_count: usize,
    pub images: [ImageItem; MAX_IMAGES],
    pub image_count: usize,
    pub fields: [FieldItem; MAX_FIELDS],
    pub field_count: usize,
    pub links: [LinkBox; MAX_LINK_BOXES],
    pub link_count: usize,
    pub form: FormInfo,
    /// The document's height in pixels.
    pub height: u32,
    /// Whether a display-list, DOM or layout-depth capacity was reached.
    pub truncated: bool,
    /// Which capacities were reached, as the `OVERFLOW_*` bits of this module
    /// and [`super::dom`]. Zero exactly when `truncated` is false.
    pub overflow: u32,
}

impl TextLayout {
    pub const fn new() -> Self {
        Self {
            runs: [TextRun::EMPTY; MAX_TEXT_RUNS],
            run_count: 0,
            rects: [RectItem::EMPTY; MAX_RECTS],
            rect_count: 0,
            images: [ImageItem::EMPTY; MAX_IMAGES],
            image_count: 0,
            fields: [FieldItem::EMPTY; MAX_FIELDS],
            field_count: 0,
            links: [LinkBox::EMPTY; MAX_LINK_BOXES],
            link_count: 0,
            form: FormInfo::EMPTY,
            height: 0,
            truncated: false,
            overflow: 0,
        }
    }

    pub fn clear(&mut self) {
        self.run_count = 0;
        self.rect_count = 0;
        self.image_count = 0;
        self.field_count = 0;
        self.link_count = 0;
        self.form = FormInfo::EMPTY;
        self.height = 0;
        self.truncated = false;
        self.overflow = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.run_count == 0
            && self.rect_count == 0
            && self.image_count == 0
            && self.field_count == 0
    }

    /// The link at a document-space point, if any.
    pub fn link_at(&self, x: usize, y: usize) -> Option<u8> {
        for link in &self.links[..self.link_count] {
            if link.contains(x, y) {
                return Some(link.link);
            }
        }
        None
    }

    /// The field at a document-space point, if any.
    pub fn field_at(&self, x: usize, y: usize) -> Option<usize> {
        for (index, field) in self.fields[..self.field_count].iter().enumerate() {
            let (x, y) = (x as i32, y as i32);
            if x >= field.x as i32
                && y >= field.y as i32
                && x < field.x as i32 + field.w as i32
                && y < field.y as i32 + field.h as i32
            {
                return Some(index);
            }
        }
        None
    }
}

impl Default for TextLayout {
    fn default() -> Self {
        Self::new()
    }
}

/// Lays `dom` out in a column `width` pixels wide.
pub fn build(dom: &Dom, sheet: &css::Stylesheet<'_>, width: usize, out: &mut TextLayout) {
    out.clear();
    let width = width.max(32);
    let mut builder = Builder {
        dom,
        sheet,
        out,
        segs: Segs::new(),
        overflow: dom.overflow(),
        max_bottom: 0,
    };
    let root = dom.root();
    if root == super::dom::NO_NODE {
        builder.finish(0);
        return;
    }
    let body = find_body(dom, root);
    let body_style = style::compute(dom, body, sheet, None);
    builder.layout_children(body, 0, 0, width, &body_style, 0);
    let height = builder.max_bottom.clamp(0, i32::MAX) as u32;
    builder.finish(height);
}

fn find_body(dom: &Dom, root: NodeId) -> NodeId {
    for (index, candidate) in dom.nodes().iter().enumerate() {
        if candidate.is_element() && candidate.tag.get(dom) == b"body" {
            return index as NodeId;
        }
    }
    root
}

struct Builder<'a, 's> {
    dom: &'a Dom,
    sheet: &'s css::Stylesheet<'s>,
    out: &'a mut TextLayout,
    /// The inline segments collected but not yet laid into lines. One list
    /// for the whole page, emptied whenever it fills.
    segs: Segs,
    overflow: u32,
    max_bottom: i32,
}

/// The column inline content is being collected into, and where it will land.
///
/// WHY this travels with the collector: a paragraph longer than the segment
/// list has to be laid into lines from inside the walk that is reading it, so
/// the walk needs the column and the line cursor, not just the list.
struct Flow {
    x: i32,
    width: usize,
    y: i32,
    pending_space: bool,
}

impl Flow {
    const fn new(x: i32, y: i32, width: usize) -> Self {
        Self {
            x,
            width,
            y,
            pending_space: false,
        }
    }
}

/// One piece of inline content.
///
/// A field or image stores only its node: the segment list is on the
/// stack at every block level, so it must not carry a whole computed
/// style. `flush_inline` recomputes the two styles it needs.
#[derive(Clone, Copy)]
enum Seg {
    Word {
        text: StrRef,
        scale: u8,
        bold: bool,
        color: u32,
        has_color: bool,
        space_before: bool,
        link: u8,
    },
    Break,
    Field {
        node: NodeId,
        link: u8,
    },
    Image {
        node: NodeId,
        link: u8,
    },
}

impl Seg {
    const EMPTY: Seg = Seg::Break;
}

struct Segs {
    items: [Seg; MAX_SEGMENTS],
    len: usize,
    overflowed: bool,
}

impl Segs {
    const fn new() -> Self {
        Self {
            items: [Seg::EMPTY; MAX_SEGMENTS],
            len: 0,
            overflowed: false,
        }
    }

    fn push(&mut self, seg: Seg) {
        if self.len < MAX_SEGMENTS {
            self.items[self.len] = seg;
            self.len += 1;
        } else {
            self.overflowed = true;
        }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.overflowed = false;
    }

    fn is_full(&self) -> bool {
        self.len >= MAX_SEGMENTS
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<'a, 's> Builder<'a, 's> {
    /// Lays out a block container's children and returns the bottom edge.
    fn layout_children(
        &mut self,
        parent: NodeId,
        x: i32,
        y: i32,
        width: usize,
        parent_style: &BoxStyle,
        depth: usize,
    ) -> i32 {
        if depth > MAX_LAYOUT_DEPTH {
            self.note(OVERFLOW_BLOCK_DEPTH);
            return y;
        }
        let mut flow = Flow::new(x, y, width);
        let ordered = self.dom.tag(parent).eq_ignore_ascii_case(b"ol");
        let mut list_index = 0usize;

        let mut child = self.dom.first_child(parent);
        while let Some(node) = child {
            let is_text = self.dom.node(node).is_text();
            let is_element = self.dom.node(node).is_element();
            if is_text {
                self.collect_text(node, parent_style, NO_LINK, &mut flow);
            } else if is_element {
                let style = style::compute(self.dom, node, self.sheet, Some(parent_style));
                if style.display == Display::None {
                } else if style.is_inline_level() {
                    self.collect_inline(node, parent_style, NO_LINK, &mut flow);
                } else {
                    self.note_overflow();
                    self.flush_inline(&mut flow);
                    flow.pending_space = false;
                    let is_item = style.display == Display::ListItem;
                    let marker = if is_item {
                        list_index += 1;
                        match style.list_style_type {
                            style::ListStyleType::None => Marker::None,
                            style::ListStyleType::Decimal => Marker::Number(list_index),
                            style::ListStyleType::Disc => {
                                if ordered {
                                    Marker::Number(list_index)
                                } else {
                                    Marker::Bullet
                                }
                            }
                        }
                    } else {
                        Marker::None
                    };
                    flow.y = self.layout_block(
                        node,
                        x,
                        flow.y,
                        width,
                        parent_style,
                        depth + 1,
                        marker,
                    );
                }
            }
            child = self.dom.next_sibling(node);
        }
        self.note_overflow();
        self.flush_inline(&mut flow);
        flow.y
    }

    /// Lays out one block-level element and returns the bottom edge.
    fn layout_block(
        &mut self,
        node: NodeId,
        x: i32,
        y: i32,
        width: usize,
        parent_style: &BoxStyle,
        depth: usize,
        marker: Marker,
    ) -> i32 {
        if depth > MAX_LAYOUT_DEPTH {
            self.note(OVERFLOW_BLOCK_DEPTH);
            return y;
        }
        let style = style::compute(self.dom, node, self.sheet, Some(parent_style));
        if style.display == Display::None {
            return y;
        }
        let tag = self.dom.tag(node);

        if tag.eq_ignore_ascii_case(b"form") && !self.out.form.present {
            self.out.form.present = true;
            if let Some(action) = self.dom.attr_ref(node, b"action") {
                self.out.form.action = action;
            }
            if self
                .dom
                .attr(node, b"method")
                .map_or(false, |m| m.eq_ignore_ascii_case(b"post"))
            {
                self.out.form.method = FormMethod::Post;
            }
        }

        let margin_top = style.margin_top.max(0) as i32;
        let margin_bottom = style.margin_bottom.max(0) as i32;
        let margin_left = style.margin_left.max(0) as i32;
        let margin_right = style.margin_right.max(0) as i32;
        let border = style.border as i32;
        let box_x = x + margin_left;
        let inner_w = (width as i32
            - margin_left
            - margin_right
            - style.padding_left as i32
            - style.padding_right as i32
            - border * 2)
            .max(32) as usize;
        let pos_offset_x = match style.position {
            super::style::Position::Relative if style.has_left => style.left as i32,
            super::style::Position::Relative if style.has_right => -(style.right as i32),
            super::style::Position::Absolute if style.has_left => style.left as i32 - box_x,
            _ => 0,
        };
        let pos_offset_y = match style.position {
            super::style::Position::Relative if style.has_top => style.top as i32,
            super::style::Position::Relative if style.has_bottom => -(style.bottom as i32),
            super::style::Position::Absolute if style.has_top => style.top as i32 - (y + margin_top),
            _ => 0,
        };
        let content_x = box_x + border + style.padding_left as i32 + pos_offset_x;
        let top = y + margin_top + pos_offset_y;

        if tag.eq_ignore_ascii_case(b"hr") {
            let rule_y = top + 6;
            self.push_rect(
                content_x,
                rule_y,
                inner_w as u16,
                1,
                0xFF_D8D8D8,
                RectKind::Rule,
            );
            let bottom = rule_y + 1 + 6 + margin_bottom;
            self.bump(bottom);
            return bottom;
        }

        if style.display == Display::Field {
            let bottom = self.layout_field(node, &style, content_x, top, inner_w);
            self.bump(bottom + margin_bottom);
            return bottom + margin_bottom;
        }

        if style.display == Display::Table {
            let bottom = self.layout_table(node, &style, content_x, top, inner_w, depth + 1);
            self.bump(bottom + margin_bottom);
            return bottom + margin_bottom;
        }

        let top_pad = top + style.padding_top as i32 + border;
        if marker != Marker::None {
            self.push_marker(marker, content_x, top_pad, &style);
        }
        let mut bottom = if self.has_only_inline_children(node) {
            self.layout_inline_children(node, content_x, top_pad, inner_w, &style, depth + 1)
        } else {
            self.layout_children(node, content_x, top_pad, inner_w, &style, depth + 1)
        };
        bottom += style.padding_bottom as i32 + border;
        if style.css.has_background {
            self.push_rect(
                box_x,
                top,
                (inner_w as i32
                    + style.padding_left as i32
                    + style.padding_right as i32
                    + border * 2) as u16,
                (bottom - top).max(1) as u16,
                style.css.background,
                RectKind::Fill,
            );
        }
        if border > 0 {
            if let Some(color) = style.border_color() {
                self.push_rect(
                    box_x,
                    top,
                    (inner_w as i32
                        + style.padding_left as i32
                        + style.padding_right as i32
                        + border * 2) as u16,
                    (bottom - top).max(1) as u16,
                    color,
                    RectKind::Outline,
                );
            }
        }
        let result = bottom + margin_bottom;
        self.bump(result);
        result
    }

    /// Lays out a container whose children are all inline content.
    fn layout_inline_children(
        &mut self,
        node: NodeId,
        x: i32,
        y: i32,
        width: usize,
        style: &BoxStyle,
        depth: usize,
    ) -> i32 {
        if depth > MAX_LAYOUT_DEPTH {
            self.note(OVERFLOW_BLOCK_DEPTH);
            return y;
        }
        let mut flow = Flow::new(x, y, width);
        let mut child = self.dom.first_child(node);
        while let Some(child_id) = child {
            let is_text = self.dom.node(child_id).is_text();
            if is_text {
                self.collect_text(child_id, style, NO_LINK, &mut flow);
            } else {
                self.collect_inline(child_id, style, NO_LINK, &mut flow);
            }
            child = self.dom.next_sibling(child_id);
        }
        self.note_overflow();
        self.flush_inline(&mut flow);
        flow.y
    }

    fn has_only_inline_children(&self, node: NodeId) -> bool {
        let mut child = self.dom.first_child(node);
        while let Some(id) = child {
            if self.dom.node(id).is_element() {
                let style = style::compute(self.dom, id, self.sheet, None);
                if style.display != Display::None && !style.is_inline_level() {
                    return false;
                }
            }
            child = self.dom.next_sibling(id);
        }
        true
    }

    fn layout_field(
        &mut self,
        node: NodeId,
        style: &BoxStyle,
        x: i32,
        y: i32,
        width: usize,
    ) -> i32 {
        let (w, h) = field_size(self.dom, node, style, width);
        let checked = self.dom.has_attr(node, b"checked");
        self.push_field(node, style, x, y, w, h, checked);
        if style.css.has_background {
            self.push_rect(x, y, w as u16, h as u16, style.css.background, RectKind::Fill);
        }
        if let Some(color) = style.border_color() {
            self.push_rect(x, y, w as u16, h as u16, color, RectKind::Outline);
        }
        y + h as i32 + style.margin_bottom as i32
    }

    fn layout_table(
        &mut self,
        node: NodeId,
        style: &BoxStyle,
        x: i32,
        y: i32,
        width: usize,
        depth: usize,
    ) -> i32 {
        if depth > MAX_LAYOUT_DEPTH {
            self.note(OVERFLOW_BLOCK_DEPTH);
            return y;
        }
        let mut rows = [super::dom::NO_NODE; 64];
        let mut row_count = 0usize;
        let mut child = self.dom.first_child(node);
        while let Some(id) = child {
            let tag = self.dom.tag(id);
            if tag.eq_ignore_ascii_case(b"tr") {
                if row_count < rows.len() {
                    rows[row_count] = id;
                    row_count += 1;
                }
            } else if tag.eq_ignore_ascii_case(b"thead")
                || tag.eq_ignore_ascii_case(b"tbody")
                || tag.eq_ignore_ascii_case(b"tfoot")
            {
                let mut inner = self.dom.first_child(id);
                while let Some(row) = inner {
                    if self.dom.tag(row).eq_ignore_ascii_case(b"tr") {
                        if row_count < rows.len() {
                            rows[row_count] = row;
                            row_count += 1;
                        }
                    }
                    inner = self.dom.next_sibling(row);
                }
            }
            child = self.dom.next_sibling(id);
        }
        if row_count == 0 {
            return y;
        }

        let mut columns = 1usize;
        for &row in &rows[..row_count] {
            let mut cells = 0usize;
            let mut cell = self.dom.first_child(row);
            while let Some(id) = cell {
                let tag = self.dom.tag(id);
                if tag == b"td" || tag == b"th" {
                    cells += 1;
                }
                cell = self.dom.next_sibling(id);
            }
            columns = columns.max(cells);
        }
        columns = columns.min(8).max(1);
        let cell_w = (width / columns).max(24);

        let mut cursor_y = y;
        for &row in &rows[..row_count] {
            let row_bottom =
                self.layout_table_row(row, x, cursor_y, cell_w, width, style, depth + 1);
            cursor_y = row_bottom;
        }
        cursor_y
    }

    fn layout_table_row(
        &mut self,
        row: NodeId,
        x: i32,
        y: i32,
        cell_w: usize,
        row_width: usize,
        table_style: &BoxStyle,
        depth: usize,
    ) -> i32 {
        if depth > MAX_LAYOUT_DEPTH {
            self.note(OVERFLOW_BLOCK_DEPTH);
            return y;
        }
        let mut column = 0usize;
        let mut row_bottom = y;
        let mut cell = self.dom.first_child(row);
        while let Some(id) = cell {
            let tag = self.dom.tag(id);
            if tag == b"td" || tag == b"th" {
                let span = self
                    .dom
                    .attr(id, b"colspan")
                    .and_then(parse_small)
                    .unwrap_or(1)
                    .clamp(1, 4);
                let w = (cell_w * span).min(row_width.saturating_sub(column * cell_w));
                let cell_x = x + (column * cell_w) as i32;
                let bottom = self.layout_block(
                    id,
                    cell_x,
                    y,
                    w.max(24),
                    table_style,
                    depth + 1,
                    Marker::None,
                );
                row_bottom = row_bottom.max(bottom);
                column += span;
            }
            cell = self.dom.next_sibling(id);
        }
        row_bottom
    }

    fn collect_inline(
        &mut self,
        node: NodeId,
        parent_style: &BoxStyle,
        parent_link: u8,
        flow: &mut Flow,
    ) {
        let style = style::compute(self.dom, node, self.sheet, Some(parent_style));
        if style.display == Display::None {
            return;
        }
        let link = self.link_of(node);
        let link = if link != NO_LINK { link } else { parent_link };
        let tag = self.dom.tag(node);

        if tag.eq_ignore_ascii_case(b"br") {
            self.push_seg(flow, Seg::Break);
            flow.pending_space = false;
            return;
        }
        if tag.eq_ignore_ascii_case(b"img") {
            self.push_seg(flow, Seg::Image { node, link });
            flow.pending_space = false;
            return;
        }
        if style.display == Display::Field {
            self.push_seg(flow, Seg::Field { node, link });
            flow.pending_space = false;
            return;
        }
        if style.preformatted {
            let mut child = self.dom.first_child(node);
            while let Some(id) = child {
                let is_text = self.dom.node(id).is_text();
                if is_text {
                    self.collect_preformatted(id, &style, link, flow);
                } else {
                    self.collect_inline(id, &style, link, flow);
                }
                child = self.dom.next_sibling(id);
            }
            return;
        }
        let mut child = self.dom.first_child(node);
        while let Some(id) = child {
            let is_text = self.dom.node(id).is_text();
            if is_text {
                self.collect_text(id, &style, link, flow);
            } else {
                self.collect_inline(id, &style, link, flow);
            }
            child = self.dom.next_sibling(id);
        }
        // WHY a separator is owed on leaving an inline element too: minified and
        // framework markup puts adjacent inline elements together with no
        // whitespace between them, and `pending_space` was only ever set from a
        // space inside a text node. `<span>Rust</span><span>Microkernel</span>`
        // therefore produced two runs but no gap, and the page read
        // "RustMicrokernel". Owing the space on both ends of the element makes
        // the boundary itself the separator, which is what the reader path
        // already does.
        flow.pending_space = true;
    }

    fn collect_text(&mut self, node: NodeId, style: &BoxStyle, link: u8, flow: &mut Flow) {
        let text_ref = self.dom.node(node).text;
        let bytes = text_ref.get(self.dom);
        if bytes.is_empty() {
            return;
        }
        if style.preformatted {
            self.collect_preformatted(node, style, link, flow);
            return;
        }
        let mut i = 0usize;
        let n = bytes.len();
        while i < n {
            let b = bytes[i];
            if is_html_space(b) {
                if !self.segs.is_empty() {
                    flow.pending_space = true;
                }
                i += 1;
                continue;
            }
            let start = i;
            while i < n && !is_html_space(bytes[i]) {
                i += 1;
            }
            let word = StrRef {
                start: text_ref.start + start as u32,
                len: (i - start).min(u16::MAX as usize) as u16,
            };
            let space_before = flow.pending_space && !self.segs.is_empty();
            flow.pending_space = false;
            self.push_seg(
                flow,
                Seg::Word {
                    text: word,
                    scale: style.scale,
                    bold: style.bold,
                    color: style.css.color,
                    has_color: style.css.has_color,
                    space_before,
                    link,
                },
            );
        }
    }

    fn collect_preformatted(&mut self, node: NodeId, style: &BoxStyle, link: u8, flow: &mut Flow) {
        let text_ref = self.dom.node(node).text;
        let bytes = text_ref.get(self.dom);
        let mut start = 0usize;
        for (index, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                if index > start {
                    self.push_seg(
                        flow,
                        Seg::Word {
                            text: StrRef {
                                start: text_ref.start + start as u32,
                                len: (index - start).min(u16::MAX as usize) as u16,
                            },
                            scale: style.scale,
                            bold: style.bold,
                            color: style.css.color,
                            has_color: style.css.has_color,
                            space_before: false,
                            link,
                        },
                    );
                }
                self.push_seg(flow, Seg::Break);
                start = index + 1;
            }
        }
        if start < bytes.len() {
            self.push_seg(
                flow,
                Seg::Word {
                    text: StrRef {
                        start: text_ref.start + start as u32,
                        len: (bytes.len() - start).min(u16::MAX as usize) as u16,
                    },
                    scale: style.scale,
                    bold: style.bold,
                    color: style.css.color,
                    has_color: style.css.has_color,
                    space_before: false,
                    link,
                },
            );
        }
    }

    /// Places collected inline segments into lines and clears the list.
    ///
    /// WHY this is the only place the line cursor moves: a collector that runs
    /// out of room mid-paragraph calls it, and the next collection resumes from
    /// the line this one ended, so a paragraph of any length flows.
    fn flush_inline(&mut self, flow: &mut Flow) -> i32 {
        if self.segs.is_empty() {
            return flow.y;
        }
        let dom = self.dom;
        let width = flow.width;
        let left = flow.x;
        let right = flow.x + width as i32;
        let mut line_x = left;
        let mut line_y = flow.y;
        let mut line_h = 0i32;
        let mut emitted = false;

        let mut index = 0usize;
        while index < self.segs.len {
            let seg = self.segs.items[index];
            index += 1;
            match seg {
                Seg::Break => {
                    line_y += line_h.max(1);
                    line_x = left;
                    line_h = 0;
                }
                Seg::Word {
                    text,
                    scale,
                    bold,
                    color,
                    has_color,
                    space_before,
                    link,
                } => {
                    let bytes = text.get(dom);
                    if bytes.is_empty() {
                        continue;
                    }
                    let word_w = font::measure(bytes, scale as usize) as i32;
                    let space_w = if space_before {
                        font::measure(b" ", scale as usize) as i32
                    } else {
                        0
                    };
                    if line_x > left && line_x + space_w + word_w > right {
                        line_y += line_h.max(1);
                        line_x = left;
                        line_h = 0;
                    } else if space_w > 0 && line_x > left {
                        line_x += space_w;
                    }
                    if word_w <= right - left {
                        let h = font::line_height(scale as usize).max(1) as i32;
                        self.push_run(
                            line_x,
                            line_y,
                            word_w.max(1) as u16,
                            h as u16,
                            scale,
                            bold,
                            color,
                            has_color,
                            text,
                            [0; 4],
                            0,
                            link,
                        );
                        line_x += word_w;
                        line_h = line_h.max(h);
                        emitted = true;
                    } else {
                        // A word wider than the column is broken at a
                        // character boundary, never inside a character.
                        let mut start = 0usize;
                        while start < bytes.len() {
                            let mut end = start;
                            let mut w = 0usize;
                            while end < bytes.len() {
                                let step = utf8_len(bytes[end]);
                                let next = (end + step).min(bytes.len());
                                let cw = font::measure(&bytes[end..next], scale as usize);
                                if w + cw > width && end > start {
                                    break;
                                }
                                w += cw;
                                end = next;
                            }
                            if end == start {
                                end = (start + 1).min(bytes.len());
                            }
                            let piece = StrRef {
                                start: text.start + start as u32,
                                len: (end - start).min(u16::MAX as usize) as u16,
                            };
                            let h = font::line_height(scale as usize).max(1) as i32;
                            self.push_run(
                                line_x,
                                line_y,
                                w.max(1) as u16,
                                h as u16,
                                scale,
                                bold,
                                color,
                                has_color,
                                piece,
                                [0; 4],
                                0,
                                link,
                            );
                            start = end;
                            emitted = true;
                            line_h = line_h.max(h);
                            if start < bytes.len() {
                                line_y += line_h.max(1);
                                line_x = left;
                                line_h = 0;
                            } else {
                                line_x += w as i32;
                            }
                        }
                    }
                }
                Seg::Field { node, link } => {
                    let style = style::compute(dom, node, self.sheet, None);
                    let (w, h) = field_size(dom, node, &style, width);
                    if line_x > left && line_x + w as i32 > right {
                        line_y += line_h.max(1);
                        line_x = left;
                        line_h = 0;
                    }
                    let checked = dom.has_attr(node, b"checked");
                    self.push_field(node, &style, line_x, line_y, w, h, checked);
                    if let Some(color) = style.border_color() {
                        self.push_rect(
                            line_x,
                            line_y,
                            w as u16,
                            h as u16,
                            color,
                            RectKind::Outline,
                        );
                    }
                    line_x += w as i32;
                    line_h = line_h.max(h as i32);
                    emitted = true;
                    self.expand_link(link, line_x - w as i32, line_y, w as i32, h as i32);
                }
                Seg::Image { node, link } => {
                    let style = style::compute(dom, node, self.sheet, None);
                    let w = if style.width > 0 {
                        (style.width as usize).min(width)
                    } else {
                        160.min(width)
                    };
                    let h = if style.height > 0 {
                        style.height as usize
                    } else {
                        120
                    };
                    if line_x > left && line_x + w as i32 > right {
                        line_y += line_h.max(1);
                        line_x = left;
                        line_h = 0;
                    }
                    let slot = self.place_image(style.src);
                    let (used_w, used_h) = self.push_image(
                        line_x,
                        line_y,
                        w.max(1),
                        h.max(1),
                        slot,
                        style.src,
                        style.alt,
                        link,
                    );
                    line_x += used_w;
                    line_h = line_h.max(used_h.max(1));
                    emitted = true;
                }
            }
        }
        self.segs.clear();
        if emitted {
            flow.y = line_y + line_h.max(1);
        }
        flow.pending_space = false;
        flow.y
    }

    fn push_marker(&mut self, marker: Marker, x: i32, y: i32, style: &BoxStyle) {
        match marker {
            Marker::None => {}
            Marker::Bullet => {
                self.push_run(
                    (x - 14).max(0),
                    y,
                    3,
                    font::line_height(style.scale as usize).max(1) as u16,
                    style.scale,
                    style.bold,
                    style.css.color,
                    style.css.has_color,
                    StrRef::EMPTY,
                    [BULLET[0], BULLET[1], BULLET[2], 0],
                    3,
                    NO_LINK,
                );
            }
            Marker::Number(value) => {
                let mut lit = [0u8; 4];
                let len = format_small_number(value, &mut lit);
                self.push_run(
                    (x - 18).max(0),
                    y,
                    font::measure(&lit[..len], style.scale as usize).max(1) as u16,
                    font::line_height(style.scale as usize).max(1) as u16,
                    style.scale,
                    style.bold,
                    style.css.color,
                    style.css.has_color,
                    StrRef::EMPTY,
                    lit,
                    len as u8,
                    NO_LINK,
                );
            }
        }
    }

    /// Reserves an image slot for `src`, or returns 255 when there is no
    /// source or the arena is full.
    fn place_image(&mut self, src: StrRef) -> u8 {
        if src.is_empty() || self.out.image_count >= MAX_IMAGES {
            return 255;
        }
        let slot = self.out.image_count as u8;
        self.out.image_count += 1;
        slot
    }

    #[allow(clippy::too_many_arguments)]
    fn push_run(
        &mut self,
        x: i32,
        y: i32,
        w: u16,
        h: u16,
        scale: u8,
        bold: bool,
        color: u32,
        has_color: bool,
        text: StrRef,
        literal: [u8; 4],
        literal_len: u8,
        link: u8,
    ) {
        if x < i16::MIN as i32 || y < i16::MIN as i32 || y > i16::MAX as i32 {
            self.note(OVERFLOW_COORDS);
            return;
        }
        if self.out.run_count >= MAX_TEXT_RUNS {
            self.note(OVERFLOW_RUNS);
            return;
        }
        let run = TextRun {
            x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            w: w.min(i16::MAX as u16),
            h,
            scale,
            bold,
            color,
            has_color,
            text,
            literal,
            literal_len,
            link,
        };
        self.out.runs[self.out.run_count] = run;
        self.out.run_count += 1;
        self.expand_link(link, x, y, w as i32, h as i32);
    }

    fn push_rect(&mut self, x: i32, y: i32, w: u16, h: u16, color: u32, kind: RectKind) {
        if w == 0 || h == 0 {
            return;
        }
        if self.out.rect_count >= MAX_RECTS {
            self.note(OVERFLOW_RECTS);
            return;
        }
        if x > i16::MAX as i32 || y > i16::MAX as i32 || y < i16::MIN as i32 {
            self.note(OVERFLOW_COORDS);
            return;
        }
        self.out.rects[self.out.rect_count] = RectItem {
            x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            w,
            h,
            color,
            kind,
        };
        self.out.rect_count += 1;
    }

    /// Records an image or falls back to its alt text when no slot could
    /// be reserved. Returns the width and height actually consumed.
    #[allow(clippy::too_many_arguments)]
    fn push_image(
        &mut self,
        x: i32,
        y: i32,
        w: usize,
        h: usize,
        slot: u8,
        src: StrRef,
        alt: StrRef,
        link: u8,
    ) -> (i32, i32) {
        if slot != 255 {
            self.out.images[slot as usize] = ImageItem {
                x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                w: w.min(u16::MAX as usize) as u16,
                h: h.min(u16::MAX as usize) as u16,
                slot,
                src,
                alt,
                link,
            };
            self.expand_link(link, x, y, w as i32, h as i32);
            return (w as i32, h as i32);
        }
        if !src.is_empty() {
            self.note(OVERFLOW_IMAGES);
        }
        if alt.is_empty() {
            return (0, 0);
        }
        let dom = self.dom;
        let bytes = alt.get(dom);
        let width = font::measure(bytes, 1).max(1) as i32;
        let height = font::line_height(1).max(1) as i32;
        self.push_run(
            x,
            y,
            width as u16,
            height as u16,
            1,
            false,
            0,
            false,
            alt,
            [0; 4],
            0,
            link,
        );
        (width, height)
    }

    fn push_field(
        &mut self,
        node: NodeId,
        style: &BoxStyle,
        x: i32,
        y: i32,
        w: usize,
        h: usize,
        checked: bool,
    ) {
        if self.out.field_count >= MAX_FIELDS {
            self.note(OVERFLOW_FIELDS);
            return;
        }
        let value = if style.field == FieldKind::Textarea {
            self.textarea_value(node)
        } else if style.field == FieldKind::Select {
            self.selected_option(node)
        } else {
            style.field_value
        };
        self.out.fields[self.out.field_count] = FieldItem {
            x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            w: w.min(u16::MAX as usize) as u16,
            h: h.min(u16::MAX as usize) as u16,
            node,
            kind: style.field,
            input: style.input_type,
            name: style.field_name,
            value,
            placeholder: style.placeholder,
            checked,
        };
        self.out.field_count += 1;
    }

    fn textarea_value(&self, node: NodeId) -> StrRef {
        self.direct_text(node)
    }

    /// The value of a select's selected option.
    ///
    /// A select with no selected option shows its first, which is what a
    /// browser does; an option's `value` wins over its label, and an
    /// option with neither carries an empty value.
    fn selected_option(&self, node: NodeId) -> StrRef {
        let mut first = StrRef::EMPTY;
        let mut child = self.dom.first_child(node);
        while let Some(id) = child {
            if self.dom.node(id).is_element() && self.dom.tag(id) == b"option" {
                let value = self
                    .dom
                    .attr_ref(id, b"value")
                    .unwrap_or_else(|| self.direct_text(id));
                if first.is_empty() {
                    first = value;
                }
                if self.dom.has_attr(id, b"selected") {
                    return value;
                }
            }
            child = self.dom.next_sibling(id);
        }
        first
    }

    fn direct_text(&self, node: NodeId) -> StrRef {
        let mut child = self.dom.first_child(node);
        while let Some(id) = child {
            if self.dom.node(id).is_text() {
                return self.dom.node(id).text;
            }
            child = self.dom.next_sibling(id);
        }
        StrRef::EMPTY
    }

    fn link_of(&self, node: NodeId) -> u8 {
        let link = self.dom.node(node).link;
        if link != NO_LINK {
            return link;
        }
        let mut cursor = self.dom.parent(node);
        while let Some(parent) = cursor {
            let link = self.dom.node(parent).link;
            if link != NO_LINK {
                return link;
            }
            cursor = self.dom.parent(parent);
        }
        NO_LINK
    }

    fn expand_link(&mut self, link: u8, x: i32, y: i32, w: i32, h: i32) {
        if link == NO_LINK {
            return;
        }
        for index in 0..self.out.link_count {
            if self.out.links[index].link == link {
                let existing = &mut self.out.links[index];
                let right = (existing.x as i32 + existing.w as i32).max(x + w);
                let bottom = (existing.y as i32 + existing.h as i32).max(y + h);
                existing.x = existing.x.min(x.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
                existing.y = existing.y.min(y.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
                existing.w = (right - existing.x as i32).clamp(0, u16::MAX as i32) as u16;
                existing.h = (bottom - existing.y as i32).clamp(0, u16::MAX as i32) as u16;
                return;
            }
        }
        if self.out.link_count >= MAX_LINK_BOXES {
            self.note(OVERFLOW_LINKS);
            return;
        }
        self.out.links[self.out.link_count] = LinkBox {
            x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            w: w.clamp(0, u16::MAX as i32) as u16,
            h: h.clamp(0, u16::MAX as i32) as u16,
            link,
        };
        self.out.link_count += 1;
    }

    /// Adds one segment, laying out what is already buffered when the buffer
    /// is full. This is what lets a paragraph of any length flow.
    fn push_seg(&mut self, flow: &mut Flow, seg: Seg) {
        if self.segs.is_full() {
            self.flush_inline(flow);
        }
        self.segs.push(seg);
    }

    /// Reports a segment the list still could not hold. The flush above makes
    /// this unreachable today; it stays because a dropped segment is a page
    /// quietly missing words, and that is not something to discover later.
    fn note_overflow(&mut self) {
        if self.segs.overflowed {
            self.note(OVERFLOW_SEGMENTS);
        }
    }

    /// Records a budget that ran out. Every one of them is also a truncation.
    fn note(&mut self, budget: u32) {
        self.overflow |= budget;
    }

    /// Publishes the display list. `truncated` is kept as the one-bit answer
    /// every existing reader asks for, and it stays true exactly when the
    /// per-budget record says something ran out.
    fn finish(self, height: u32) {
        let overflow = self.overflow;
        self.out.height = height;
        self.out.overflow = overflow;
        self.out.truncated = overflow != 0;
    }

    fn bump(&mut self, bottom: i32) {
        self.max_bottom = self.max_bottom.max(bottom);
    }
}

/// The width and height a form control occupies.
fn field_size(dom: &Dom, node: NodeId, style: &BoxStyle, width: usize) -> (usize, usize) {
    match style.field {
        FieldKind::Input
            if style.input_type == InputType::Checkbox
                || style.input_type == InputType::Radio =>
        {
            (TOGGLE_FIELD_W.min(width), TOGGLE_FIELD_W)
        }
        FieldKind::Input
            if style.input_type == InputType::Submit || style.input_type == InputType::Reset =>
        {
            let label = style.field_value.get(dom);
            let measured = font::measure(label, 1) + 24;
            (measured.max(72).min(width), 40)
        }
        FieldKind::Button => {
            let label = if style.field_value.is_empty() {
                button_text(dom, node)
            } else {
                style.field_value.get(dom)
            };
            let measured = font::measure(label, 1) + 24;
            (measured.max(72).min(width), 40)
        }
        FieldKind::Textarea => (
            if style.width > 0 {
                (style.width as usize).min(width)
            } else {
                width
            },
            if style.height > 0 { style.height as usize } else { 96 },
        ),
        _ => (
            if style.width > 0 {
                (style.width as usize).min(width)
            } else {
                DEFAULT_FIELD_W.min(width)
            },
            if style.height > 0 { style.height as usize } else { 40 },
        ),
    }
}

fn button_text<'a>(dom: &'a Dom, node: NodeId) -> &'a [u8] {
    let mut child = dom.first_child(node);
    while let Some(id) = child {
        if dom.node(id).is_text() {
            return dom.text_of(id);
        }
        child = dom.next_sibling(id);
    }
    b""
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    None,
    Bullet,
    Number(usize),
}

fn is_html_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c')
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

fn parse_small(raw: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    let mut any = false;
    for &b in raw {
        if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((b - b'0') as usize);
            any = true;
        } else {
            break;
        }
    }
    if any {
        Some(value)
    } else {
        None
    }
}

fn format_small_number(value: usize, out: &mut [u8; 4]) -> usize {
    let mut digits = [0u8; 3];
    let mut n = value.min(999);
    let mut count = 0usize;
    if n == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while n > 0 && count < 3 {
            digits[count] = b'0' + (n % 10) as u8;
            n /= 10;
            count += 1;
        }
    }
    let mut len = 0usize;
    for i in (0..count).rev() {
        out[len] = digits[i];
        len += 1;
    }
    out[len] = b'.';
    len + 1
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn layout_html(html: &[u8], width: usize) -> (Dom, TextLayout) {
        let mut dom = Dom::new();
        dom.parse(html);
        let sheet = css::parse_stylesheet(dom.css());
        let mut out = TextLayout::new();
        build(&dom, &sheet, width, &mut out);
        (dom, out)
    }

    fn run_text(dom: &Dom, run: &TextRun) -> std::vec::Vec<u8> {
        run.bytes(dom).to_vec()
    }

    fn all_text(dom: &Dom, out: &TextLayout) -> std::vec::Vec<u8> {
        let mut text = std::vec::Vec::new();
        for run in &out.runs[..out.run_count] {
            if !text.is_empty() {
                text.push(b' ');
            }
            text.extend_from_slice(&run_text(dom, run));
        }
        text
    }

    #[test]
    fn a_heading_and_a_paragraph_stack_vertically() {
        let (dom, out) = layout_html(b"<h1>Title</h1><p>Body text.</p>", 300);
        assert!(out.run_count >= 2);
        let title = &out.runs[0];
        let body = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"Body")
            .unwrap();
        assert!(body.y > title.y);
        assert!(title.scale >= body.scale);
        assert!(out.height > 0);
    }

    #[test]
    fn adjacent_inline_elements_separate_with_a_space() {
        // Minified markup puts the two spans together with no whitespace between
        // them, so the element boundary itself has to become the separator.
        let (dom, out) = layout_html(b"<p><span>Rust</span><span>Microkernel</span></p>", 600);
        let left = out.runs[..out.run_count]
            .iter()
            .find(|r| run_text(&dom, r) == b"Rust")
            .expect("Rust run");
        let right = out.runs[..out.run_count]
            .iter()
            .find(|r| run_text(&dom, r) == b"Microkernel")
            .expect("Microkernel run");
        assert_eq!(left.y, right.y, "both runs stay on one line");
        assert!(
            right.x > left.x + left.w as i16,
            "adjacent inline elements must not touch: Rust ends at {}, Microkernel starts at {}",
            left.x + left.w as i16,
            right.x
        );
    }

    #[test]
    fn words_wrap_at_the_column_edge() {
        let (dom, out) = layout_html(
            b"<p>alpha beta gamma delta epsilon zeta eta theta iota kappa</p>",
            80,
        );
        let mut ys = std::vec::Vec::new();
        for run in &out.runs[..out.run_count] {
            if !ys.contains(&run.y) {
                ys.push(run.y);
            }
        }
        assert!(ys.len() > 1, "text did not wrap: {ys:?}");
        for run in &out.runs[..out.run_count] {
            assert!(run.x.max(0) as usize + run.w as usize <= 80 + 60);
        }
        let text = all_text(&dom, &out);
        assert!(text.windows(5).any(|w| w == b"alpha"));
        assert!(text.windows(5).any(|w| w == b"kappa"));
    }

    #[test]
    fn an_anchor_produces_a_link_box() {
        let (dom, out) = layout_html(b"<p>See <a href='/docs'>the docs</a> now.</p>", 400);
        assert_eq!(out.link_count, 1);
        let link = out.links[0];
        assert_eq!(dom.link_href(link.link), Some(&b"/docs"[..]));
        assert!(out.link_at(link.x as usize + 1, link.y as usize + 1).is_some());
    }

    #[test]
    fn a_list_emits_a_marker_for_each_item() {
        let (dom, out) = layout_html(b"<ul><li>one</li><li>two</li></ul>", 300);
        let markers: std::vec::Vec<_> = out.runs[..out.run_count]
            .iter()
            .filter(|r| r.literal_len > 0)
            .map(|r| run_text(&dom, r))
            .collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0], BULLET);
        let (ordered_dom, ordered) = layout_html(b"<ol><li>one</li><li>two</li></ol>", 300);
        let numbers: std::vec::Vec<_> = ordered.runs[..ordered.run_count]
            .iter()
            .filter(|r| r.literal_len > 0)
            .map(|r| run_text(&ordered_dom, r))
            .collect();
        assert_eq!(numbers, std::vec![b"1.".to_vec(), b"2.".to_vec()]);
    }

    #[test]
    fn a_table_places_cells_in_columns() {
        let (dom, out) = layout_html(
            b"<table><tr><td>left</td><td>right</td></tr></table>",
            400,
        );
        let left = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"left")
            .unwrap();
        let right = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"right")
            .unwrap();
        assert!(right.x > left.x, "cells share a column");
        assert_eq!(left.y, right.y);
        assert!(out.rect_count >= 2, "table cells have no borders");
    }

    #[test]
    fn a_form_control_is_a_field_with_its_name() {
        let (dom, out) = layout_html(
            b"<form action='/search' method='post'><input name='q' value='rust'></form>",
            400,
        );
        assert!(out.form.present);
        assert_eq!(out.form.method, FormMethod::Post);
        assert_eq!(out.field_count, 1);
        let field = out.fields[0];
        assert_eq!(field.kind, FieldKind::Input);
        assert_eq!(field.name.get(&dom), b"q");
        assert_eq!(field.value.get(&dom), b"rust");
        assert!(out.field_at(field.x as usize + 1, field.y as usize + 1).is_some());
        assert!(!out.form.action.get(&dom).is_empty());
    }

    #[test]
    fn a_field_keeps_the_node_it_came_from_and_its_form() {
        let (dom, out) = layout_html(
            b"<form action='/one'><input name='a'></form><form action='/two'><input name='b'></form>",
            400,
        );
        assert_eq!(out.field_count, 2);
        for field in &out.fields[..out.field_count] {
            assert_eq!(dom.tag(field.node), b"input");
            let mut cursor = dom.parent(field.node);
            let mut form = None;
            while let Some(id) = cursor {
                if dom.tag(id) == b"form" {
                    form = Some(id);
                    break;
                }
                cursor = dom.parent(id);
            }
            assert!(form.is_some(), "the field lost its form");
        }
        let second = out.fields[1];
        let mut cursor = dom.parent(second.node);
        while let Some(id) = cursor {
            if dom.tag(id) == b"form" {
                assert_eq!(dom.attr(id, b"action"), Some(&b"/two"[..]));
                break;
            }
            cursor = dom.parent(id);
        }
    }

    #[test]
    fn a_select_shows_its_selected_option() {
        let (dom, out) = layout_html(
            b"<select name='size'><option value='s'>Small</option><option value='m' selected>Medium</option></select>",
            400,
        );
        assert_eq!(out.field_count, 1);
        let field = out.fields[0];
        assert_eq!(field.kind, FieldKind::Select);
        assert_eq!(field.value.get(&dom), b"m");
    }

    #[test]
    fn text_runs_are_ordered_by_their_top_edge() {
        let (_dom, out) = layout_html(
            b"<h1>Title</h1><p>One paragraph with enough words to wrap at the column edge.</p><ul><li>item</li></ul><table><tr><td>a</td><td>b</td></tr></table>",
            160,
        );
        let mut previous = i32::MIN;
        for run in &out.runs[..out.run_count] {
            assert!(
                run.y as i32 >= previous,
                "run at {} follows one at {}",
                run.y,
                previous
            );
            previous = run.y as i32;
        }
    }

    #[test]
    fn a_background_and_border_become_rectangles() {
        let (_dom, out) = layout_html(
            b"<div style='background:#ffeeaa;border:1px solid #333333'>box</div>",
            300,
        );
        assert!(out.rect_count >= 2);
        let mut fill = false;
        let mut outline = false;
        for rect in &out.rects[..out.rect_count] {
            fill |= rect.kind == RectKind::Fill && rect.color == 0xFF_FFEEAA;
            outline |= rect.kind == RectKind::Outline;
        }
        assert!(fill, "the background was not painted");
        assert!(outline, "the border was not painted");
    }

    #[test]
    fn an_hidden_element_is_not_in_the_display_list() {
        let (dom, out) = layout_html(
            b"<p>shown</p><p hidden>secret</p><input type='hidden' name='csrf' value='x'>",
            300,
        );
        let text = all_text(&dom, &out);
        assert!(text.windows(5).any(|w| w == b"shown"));
        assert!(!text.windows(6).any(|w| w == b"secret"));
        assert_eq!(out.field_count, 0);
    }

    #[test]
    fn an_image_claims_one_slot_and_keeps_its_alt_text() {
        let (dom, out) = layout_html(
            b"<p><img src='/a.png' alt='A picture' width='120' height='80'></p>",
            300,
        );
        assert_eq!(out.image_count, 1);
        let image = out.images[0];
        assert_eq!(image.w, 120);
        assert_eq!(image.h, 80);
        assert_eq!(image.src.get(&dom), b"/a.png");
        assert_eq!(image.alt.get(&dom), b"A picture");
    }

    #[test]
    fn a_br_ends_the_line() {
        let (dom, out) = layout_html(b"<p>one<br>two</p>", 300);
        let one = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"one")
            .unwrap();
        let two = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"two")
            .unwrap();
        assert!(two.y > one.y);
    }

    #[test]
    fn preformatted_text_keeps_its_lines() {
        let (dom, out) = layout_html(b"<pre>a b\nc  d</pre>", 300);
        let first = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"a b")
            .unwrap();
        let second = out
            .runs
            .iter()
            .take(out.run_count)
            .find(|r| run_text(&dom, r) == b"c  d")
            .unwrap();
        assert!(second.y > first.y);
    }

    #[test]
    fn an_empty_document_lays_out_to_nothing() {
        let (_dom, out) = layout_html(b"", 300);
        assert!(out.is_empty());
        assert_eq!(out.height, 0);
        assert!(!out.truncated);
    }

    #[test]
    fn a_variety_of_form_controls_are_laid_out() {
        let (dom, out) = layout_html(
            b"<form action='/submit' method='post'>\
              <input type='text' name='uname' value='alice'>\
              <input type='password' name='pass' value='secret'>\
              <input type='checkbox' name='agree' checked>\
              <input type='radio' name='plan' value='pro' checked>\
              <textarea name='bio'>hello world</textarea>\
              <select name='role'><option value='user'>User</option><option value='admin' selected>Admin</option></select>\
              <button type='submit'>Save</button>\
              </form>",
            400,
        );
        assert!(out.form.present);
        assert_eq!(out.form.method, FormMethod::Post);
        assert_eq!(out.field_count, 7);

        // Text input
        let text_field = out.fields[0];
        assert_eq!(text_field.kind, FieldKind::Input);
        assert_eq!(text_field.input, InputType::Text);
        assert_eq!(text_field.name.get(&dom), b"uname");
        assert_eq!(text_field.value.get(&dom), b"alice");

        // Password input
        let pass_field = out.fields[1];
        assert_eq!(pass_field.kind, FieldKind::Input);
        assert_eq!(pass_field.input, InputType::Password);
        assert_eq!(pass_field.value.get(&dom), b"secret");

        // Checkbox
        let check_field = out.fields[2];
        assert_eq!(check_field.input, InputType::Checkbox);
        assert!(check_field.checked);

        // Radio
        let radio_field = out.fields[3];
        assert_eq!(radio_field.input, InputType::Radio);
        assert!(radio_field.checked);

        // Textarea
        let area_field = out.fields[4];
        assert_eq!(area_field.kind, FieldKind::Textarea);
        assert_eq!(area_field.value.get(&dom), b"hello world");

        // Select
        let select_field = out.fields[5];
        assert_eq!(select_field.kind, FieldKind::Select);
        assert_eq!(select_field.value.get(&dom), b"admin");

        // Submit Button
        let btn_field = out.fields[6];
        assert_eq!(btn_field.kind, FieldKind::Button);

        // Hit testing for all fields
        for (i, f) in out.fields[..out.field_count].iter().enumerate() {
            assert_eq!(
                out.field_at(f.x as usize + 2, f.y as usize + 2),
                Some(i),
                "field {} hit test failed",
                i
            );
        }
    }

    #[test]
    fn hit_testing_returns_none_outside_elements() {
        let (_dom, out) = layout_html(
            b"<p><a href='/link'>click</a></p><input name='test'>",
            400,
        );
        assert!(out.link_count > 0);
        assert!(out.field_count > 0);

        // Points far outside the document content
        assert_eq!(out.link_at(1000, 1000), None);
        assert_eq!(out.field_at(1000, 1000), None);
    }

    #[test]
    fn layout_truncation_is_recorded() {
        let mut dom = Dom::new();
        dom.parse(b"<p>hello</p>");
        let sheet = css::parse_stylesheet(b"");
        let mut out = TextLayout::new();
        build(&dom, &sheet, 400, &mut out);
        assert!(!out.truncated);

        // If a dom is truncated, layout records truncation
        let mut truncated_dom = Dom::new();
        // A huge body that exceeds dom node or text bounds
        let mut huge = [b'a'; 4000];
        huge[0..3].copy_from_slice(b"<p>");
        huge[3996..4000].copy_from_slice(b"</p>");
        truncated_dom.parse(&huge);
        let mut out2 = TextLayout::new();
        build(&truncated_dom, &sheet, 400, &mut out2);
        // The display list reflects whether dom or layout reached capacity
        assert_eq!(out2.truncated, truncated_dom.truncated());
    }

    /// WHY this is the test for the whole feature: `truncated` alone told a
    /// reader "shown in part" and told whoever maintains the engine nothing
    /// about which of thirteen budgets ran out, so no budget ever got raised.
    #[test]
    fn a_deeper_page_than_the_layout_descends_names_the_block_depth() {
        let mut html = std::vec::Vec::new();
        for _ in 0..40 {
            html.extend_from_slice(b"<div>");
        }
        html.extend_from_slice(b"bottom");
        let (_dom, out) = layout_html(&html, 300);
        assert!(out.truncated);
        assert_eq!(out.overflow & OVERFLOW_BLOCK_DEPTH, OVERFLOW_BLOCK_DEPTH);
        assert!(overflow_note(out.overflow).contains("nests deeper"));
    }

    /// WHY: a paragraph longer than the segment buffer used to lose every word
    /// after the last one the buffer could hold, which is what a real article
    /// is. The buffer is emptied as it fills now, so the paragraph flows to its
    /// end and nothing is reported.
    #[test]
    fn a_paragraph_longer_than_the_segment_buffer_flows_to_the_end() {
        let words = MAX_SEGMENTS * 4;
        let mut html = std::vec::Vec::new();
        html.extend_from_slice(b"<p>");
        for index in 0..words {
            if index > 0 {
                html.push(b' ');
            }
            html.extend_from_slice(b"word");
        }
        html.extend_from_slice(b"</p>");
        let (dom, out) = layout_html(&html, 300);
        assert_eq!(out.overflow, 0, "reported: {}", overflow_note(out.overflow));
        let laid_out = out
            .runs
            .iter()
            .take(out.run_count)
            .filter(|r| run_text(&dom, r) == b"word")
            .count();
        assert_eq!(laid_out, words);
        let first = out.runs.iter().take(out.run_count).next().unwrap();
        let last = out.runs.iter().take(out.run_count).last().unwrap();
        assert!(last.y > first.y, "the paragraph never wrapped");
    }

    /// WHY: component markup nests twenty and thirty levels deep, and the bound
    /// this engine held itself to was sixteen. It went up because the segment
    /// list stopped costing a buffer at every level, not because the stack grew.
    #[test]
    fn a_page_nested_past_the_previous_bound_is_laid_out() {
        let mut html = std::vec::Vec::new();
        // A block level costs two frames, the element and its child walk, so
        // twelve divs sit past the sixteen this engine used to hold itself to.
        for _ in 0..MAX_LAYOUT_DEPTH / 2 - 4 {
            html.extend_from_slice(b"<div>");
        }
        html.extend_from_slice(b"<p>deep</p>");
        let (dom, out) = layout_html(&html, 300);
        assert_eq!(out.overflow, 0, "reported: {}", overflow_note(out.overflow));
        let text = all_text(&dom, &out);
        assert!(text.windows(4).any(|w| w == b"deep"));
    }

    #[test]
    fn a_document_that_ran_out_is_reported_by_the_display_list() {
        let mut dom = Dom::new();
        // Longer than the document text arena holds, so the tree reports
        // before the layout has any reason to.
        let mut huge = std::vec::Vec::new();
        huge.extend_from_slice(b"<p>");
        huge.resize(b"<p>".len() + crate::layout::dom::MAX_TEXT_BYTES + 4096, b'a');
        huge.extend_from_slice(b"</p>");
        dom.parse(&huge);
        assert_eq!(dom.overflow() & crate::layout::dom::OVERFLOW_TEXT, crate::layout::dom::OVERFLOW_TEXT);
        let sheet = css::parse_stylesheet(dom.css());
        let mut out = TextLayout::new();
        build(&dom, &sheet, 300, &mut out);
        assert_eq!(out.overflow & dom.overflow(), dom.overflow());
        assert!(overflow_note(out.overflow).contains("more words"));
    }

    /// WHY the width is checked for every reason: the note is drawn on one
    /// line of the page, and a reason wider than the column is cut mid-word --
    /// the exact failure the note exists to avoid.
    #[test]
    fn every_overflow_reason_fits_the_line_it_is_drawn_on() {
        // The reader's column at the narrowest phone it runs on.
        let column = 720 - 2 * crate::CONTENT_PADDING;
        for (bit, reason) in OVERFLOW_BUDGETS {
            let width = font::measure(reason.as_bytes(), 1) as usize;
            assert!(width <= column, "budget {:#x} needs {} px", bit, width);
        }
        let width = font::measure(
            b"The page reached a layout capacity; it is shown in part.",
            1,
        ) as usize;
        assert!(width <= column);
    }

    #[test]
    fn the_note_names_the_worst_budget_and_only_that_one() {
        let worst = crate::layout::dom::OVERFLOW_NODES;
        let several = worst | OVERFLOW_RECTS | OVERFLOW_COORDS;
        assert!(overflow_note(several).starts_with("The document is larger"));
        assert!(overflow_note(OVERFLOW_RECTS).starts_with("The page has more panels"));
        assert_eq!(
            overflow_note(0),
            "The page reached a layout capacity; it is shown in part."
        );
    }

    /// WHY: the table is hand-written from two modules' constants. Two
    /// budgets sharing a bit would make one of them unraisable, silently.
    #[test]
    fn each_budget_owns_its_own_bit() {
        for (index, (bit, reason)) in OVERFLOW_BUDGETS.iter().enumerate() {
            assert!(*bit != 0, "budget {} names no bit", index);
            assert!(!reason.is_empty());
            for (other, _) in &OVERFLOW_BUDGETS[index + 1..] {
                assert_eq!(bit & other, 0, "budget {:#x} is claimed twice", bit);
            }
        }
    }
}

