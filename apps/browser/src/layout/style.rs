//! The layout engine's computed style.
//!
//! The reader's CSS subset ([`crate::css`]) answers five questions about a
//! block: ink, background, type rung, top margin and left inset. A box
//! layout needs more: whether an element is a block or inline, its own
//! margins on all four sides, padding, a border and the identity of a form
//! control.
//!
//! This layer starts from the element's user-agent defaults, applies the
//! author's cascade, and then inherits the properties CSS inherits. The
//! parser's own subset remains the authority on author declarations; a
//! property it does not parse is not invented here.

use super::dom::{Dom, NodeId, StrRef};
use crate::css;

/// The deepest ancestor chain the cascade considers. A selector in the
/// supported subset has at most three parts, so a deeper chain cannot
/// change a match unless the page nests more than this.
pub const MAX_ANCESTORS: usize = 24;

/// How an element participates in layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Block,
    Inline,
    ListItem,
    Table,
    TableRow,
    TableCell,
    Field,
}

/// The form control an element is, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    None,
    Input,
    Textarea,
    Select,
    Button,
}

/// The input type that changes a field's height and behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputType {
    Text,
    Password,
    Checkbox,
    Radio,
    Submit,
    Reset,
    Hidden,
    Other,
}

/// CSS text alignment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// CSS list marker type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListStyleType {
    Disc,
    Decimal,
    None,
}

/// CSS whitespace handling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WhiteSpace {
    Normal,
    Nowrap,
    Pre,
}

/// CSS box positioning model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    Static,
    Relative,
    Absolute,
}

/// The maximum number of unsupported properties recorded per element.
pub const MAX_UNSUPPORTED_NOTICES: usize = 4;

/// Diagnostic notice for an unsupported CSS property.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnsupportedStyleNotice {
    pub property: [u8; 32],
    pub len: u8,
}

impl UnsupportedStyleNotice {
    pub const EMPTY: Self = Self {
        property: [0u8; 32],
        len: 0,
    };

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.property[..self.len as usize]).unwrap_or("")
    }
}

/// The computed box properties for one element.
#[derive(Clone, Copy, Debug)]
pub struct BoxStyle {
    /// The author cascade's answer for the five reader properties.
    pub css: css::ComputedStyle,
    pub display: Display,
    /// The type rung: one or two. Always at least one.
    pub scale: u8,
    pub bold: bool,
    pub margin_top: i16,
    pub margin_bottom: i16,
    pub margin_left: i16,
    pub margin_right: i16,
    pub padding_top: i16,
    pub padding_bottom: i16,
    pub padding_left: i16,
    pub padding_right: i16,
    /// Border thickness in pixels; zero for none.
    pub border: u8,
    /// Whether this is an `<li>` and needs a marker.
    pub list_item: bool,
    /// Whether whitespace and newlines are literal.
    pub preformatted: bool,
    /// Whether the element is not rendered.
    pub hidden: bool,
    /// Width and height attributes, zero when absent.
    pub width: u16,
    pub height: u16,
    pub field: FieldKind,
    pub input_type: InputType,
    pub field_name: StrRef,
    pub field_value: StrRef,
    pub placeholder: StrRef,
    pub src: StrRef,
    pub alt: StrRef,
    pub text_align: TextAlign,
    pub list_style_type: ListStyleType,
    pub white_space: WhiteSpace,
    pub position: Position,
    pub top: i16,
    pub left: i16,
    pub right: i16,
    pub bottom: i16,
    pub has_top: bool,
    pub has_left: bool,
    pub has_right: bool,
    pub has_bottom: bool,
    pub unsupported: [UnsupportedStyleNotice; MAX_UNSUPPORTED_NOTICES],
    pub unsupported_count: u8,
}

impl BoxStyle {
    pub const EMPTY: Self = Self {
        css: css::ComputedStyle::DEFAULT,
        display: Display::Block,
        scale: 1,
        bold: false,
        margin_top: 0,
        margin_bottom: 0,
        margin_left: 0,
        margin_right: 0,
        padding_top: 0,
        padding_bottom: 0,
        padding_left: 0,
        padding_right: 0,
        border: 0,
        list_item: false,
        preformatted: false,
        hidden: false,
        width: 0,
        height: 0,
        field: FieldKind::None,
        input_type: InputType::Text,
        field_name: StrRef::EMPTY,
        field_value: StrRef::EMPTY,
        placeholder: StrRef::EMPTY,
        src: StrRef::EMPTY,
        alt: StrRef::EMPTY,
        text_align: TextAlign::Left,
        list_style_type: ListStyleType::Disc,
        white_space: WhiteSpace::Normal,
        position: Position::Static,
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        has_top: false,
        has_left: false,
        has_right: false,
        has_bottom: false,
        unsupported: [UnsupportedStyleNotice::EMPTY; MAX_UNSUPPORTED_NOTICES],
        unsupported_count: 0,
    };

    pub fn record_unsupported(&mut self, prop: &[u8]) {
        if (self.unsupported_count as usize) < MAX_UNSUPPORTED_NOTICES {
            let mut notice = UnsupportedStyleNotice::EMPTY;
            let n = prop.len().min(notice.property.len());
            notice.property[..n].copy_from_slice(&prop[..n]);
            notice.len = n as u8;
            self.unsupported[self.unsupported_count as usize] = notice;
            self.unsupported_count += 1;
        }
    }

    pub fn has_unsupported(&self) -> bool {
        self.unsupported_count > 0
    }

    pub fn is_inline(&self) -> bool {
        matches!(self.display, Display::Inline)
    }

    pub fn is_inline_level(&self) -> bool {
        matches!(self.display, Display::Inline | Display::Field)
    }

    pub fn text_color(&self) -> Option<u32> {
        if self.css.has_color {
            Some(self.css.color)
        } else {
            None
        }
    }

    pub fn background(&self) -> Option<u32> {
        if self.css.has_background {
            Some(self.css.background)
        } else {
            None
        }
    }

    pub fn border_color(&self) -> Option<u32> {
        if self.border > 0 {
            Some(if self.css.has_border {
                self.css.border_color
            } else {
                0xFF_D0D0D0
            })
        } else {
            None
        }
    }
}

/// The element as a CSS selector compound.
pub fn compound<'a>(dom: &'a Dom, node: NodeId) -> css::Compound<'a> {
    css::Compound {
        tag: dom.tag(node),
        class: dom.attr(node, b"class").unwrap_or(b""),
        id: dom.attr(node, b"id").unwrap_or(b""),
    }
}

/// Computes the style of `node`, inheriting from `parent` where CSS says
/// a property inherits.
pub fn compute(
    dom: &Dom,
    node: NodeId,
    sheet: &css::Stylesheet<'_>,
    parent: Option<&BoxStyle>,
) -> BoxStyle {
    let tag = dom.tag(node);
    let mut style = user_agent_style(dom, node, tag);

    let mut path = [css::Compound::EMPTY; MAX_ANCESTORS];
    let mut path_len = 0usize;
    let mut cursor = dom.parent(node);
    while let Some(ancestor) = cursor {
        if dom.node(ancestor).is_element() && path_len < MAX_ANCESTORS {
            path[path_len] = compound(dom, ancestor);
            path_len += 1;
        }
        cursor = dom.parent(ancestor);
    }
    // `css::cascade` expects the outermost ancestor first: its matcher
    // walks the slice from the end toward the root.
    let mut ancestors = [css::Compound::EMPTY; MAX_ANCESTORS];
    for index in 0..path_len {
        ancestors[index] = path[path_len - 1 - index];
    }
    let inline = dom.attr(node, b"style");
    let inherited_ua = style.css;
    let cascaded = css::cascade(sheet, compound(dom, node), &ancestors[..path_len], inline);
    style.css = cascaded;
    // A user-agent background (the code block surface) survives when the
    // author did not set one; an author declaration always wins.
    if !cascaded.has_background && inherited_ua.has_background {
        style.css.background = inherited_ua.background;
        style.css.has_background = true;
    }

    if cascaded.font_scale > 0 {
        style.scale = cascaded.font_scale;
    }
    if cascaded.margin_top > 0 {
        style.margin_top = cascaded.margin_top as i16;
    }
    if cascaded.padding_left > 0 {
        style.padding_left = cascaded.padding_left as i16;
    }
    if cascaded.has_border {
        style.border = 1;
    }

    if let Some(parent) = parent {
        // Colour and font size inherit; a UA rung (a heading) wins over
        // the parent's rung, and an author declaration wins over both.
        if !cascaded.has_color {
            style.css.color = parent.css.color;
            style.css.has_color = parent.css.has_color;
        }
        if cascaded.font_scale == 0 && style.scale == 1 {
            style.scale = parent.scale;
        }
        style.bold |= parent.bold;
        // CSS text-align, list-style-type, and white-space inherit.
        style.text_align = parent.text_align;
        style.list_style_type = parent.list_style_type;
        style.white_space = parent.white_space;
        style.preformatted |= parent.preformatted;
    }

    // Apply author inline declarations for extended box properties and record unsupported styles.
    apply_declarations(&mut style, inline);

    if dom.has_attr(node, b"hidden") {
        style.hidden = true;
        style.display = Display::None;
    }
    if style.scale == 0 {
        style.scale = 1;
    }
    style.scale = style.scale.min(2);
    style
}

fn user_agent_style(dom: &Dom, node: NodeId, tag: &[u8]) -> BoxStyle {
    let mut style = BoxStyle::EMPTY;
    style.display = display_for(tag);
    style.scale = if tag == b"h1" { 2 } else { 1 };

    match tag {
        b"h1" => {
            style.margin_top = 20;
            style.margin_bottom = 12;
            style.bold = true;
        }
        b"h2" => {
            style.margin_top = 18;
            style.margin_bottom = 8;
            style.bold = true;
        }
        b"h3" | b"h4" | b"h5" | b"h6" => {
            style.margin_top = 14;
            style.margin_bottom = 6;
            style.bold = true;
        }
        b"p" => {
            style.margin_top = 8;
            style.margin_bottom = 8;
        }
        b"ul" | b"ol" => {
            style.margin_top = 8;
            style.margin_bottom = 8;
            style.padding_left = 24;
            style.list_style_type = if tag == b"ol" {
                ListStyleType::Decimal
            } else {
                ListStyleType::Disc
            };
        }
        b"li" => {
            style.margin_top = 2;
            style.margin_bottom = 2;
            style.list_item = true;
        }
        b"blockquote" => {
            style.margin_top = 12;
            style.margin_bottom = 12;
            style.padding_left = 16;
            style.padding_right = 16;
            style.border = 1;
        }
        b"pre" => {
            style.margin_top = 10;
            style.margin_bottom = 10;
            style.padding_left = 10;
            style.padding_right = 10;
            style.padding_top = 8;
            style.padding_bottom = 8;
            style.preformatted = true;
            style.white_space = WhiteSpace::Pre;
            style.css.has_background = true;
            style.css.background = 0xFF_F2F2F2;
        }
        b"hr" => {
            style.margin_top = 12;
            style.margin_bottom = 12;
        }
        b"table" => {
            style.margin_top = 8;
            style.margin_bottom = 8;
        }
        b"td" | b"th" => {
            style.padding_top = 4;
            style.padding_bottom = 4;
            style.padding_left = 6;
            style.padding_right = 6;
            style.border = 1;
            if tag == b"th" {
                style.bold = true;
            }
        }
        b"figure" | b"form" | b"fieldset" => {
            style.margin_top = 8;
            style.margin_bottom = 8;
            if tag == b"fieldset" {
                style.border = 1;
            }
        }
        b"img" => {
            style.margin_top = 4;
            style.margin_bottom = 4;
        }
        b"strong" | b"b" => style.bold = true,
        b"input" | b"select" | b"button" | b"textarea" => {
            style.field = match tag {
                b"textarea" => FieldKind::Textarea,
                b"select" => FieldKind::Select,
                b"button" => FieldKind::Button,
                _ => FieldKind::Input,
            };
            style.border = 1;
            style.padding_left = 10;
            style.padding_right = 10;
            style.padding_top = 8;
            style.padding_bottom = 8;
            style.margin_top = 4;
            style.margin_bottom = 4;
            if tag == b"textarea" {
                style.height = 96;
            } else if tag == b"select" {
                style.height = 40;
            } else if tag == b"button" {
                style.height = 40;
            } else {
                style.height = 40;
            }
            if tag == b"input" {
                let kind = dom.attr(node, b"type").unwrap_or(b"text");
                style.input_type = input_type(kind);
                if style.input_type == InputType::Hidden {
                    style.hidden = true;
                    style.display = Display::None;
                }
            }
            style.field_name = attr_ref(dom, node, b"name");
            style.field_value = attr_ref(dom, node, b"value");
            style.placeholder = attr_ref(dom, node, b"placeholder");
        }
        _ => {}
    }

    if tag == b"img" {
        style.src = attr_ref(dom, node, b"src");
        style.alt = attr_ref(dom, node, b"alt");
    }
    style.width = dimension(dom.attr(node, b"width"));
    style.height = if style.height > 0 {
        style.height
    } else {
        dimension(dom.attr(node, b"height"))
    };
    style
}

fn display_for(tag: &[u8]) -> Display {
    match tag {
        b"head" | b"script" | b"style" | b"title" | b"meta" | b"link" | b"base" | b"template"
        | b"noscript" | b"iframe" | b"canvas" | b"svg" => Display::None,
        b"html" | b"body" | b"div" | b"p" | b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6"
        | b"ul" | b"ol" | b"dl" | b"dt" | b"dd" | b"blockquote" | b"pre" | b"hr" | b"figure"
        | b"figcaption" | b"form" | b"fieldset" | b"header" | b"footer" | b"main" | b"nav"
        | b"section" | b"article" | b"aside" | b"address" | b"thead" | b"tbody"
        | b"tfoot" | b"caption" | b"colgroup" => Display::Block,
        b"table" => Display::Table,
        b"li" => Display::ListItem,
        b"tr" => Display::TableRow,
        b"td" | b"th" => Display::TableCell,
        b"input" | b"select" | b"button" | b"textarea" => Display::Field,
        _ => Display::Inline,
    }
}

fn input_type(raw: &[u8]) -> InputType {
    match raw {
        b"password" => InputType::Password,
        b"checkbox" => InputType::Checkbox,
        b"radio" => InputType::Radio,
        b"submit" => InputType::Submit,
        b"reset" => InputType::Reset,
        b"hidden" => InputType::Hidden,
        b"text" | b"" => InputType::Text,
        _ => InputType::Other,
    }
}

fn attr_ref(dom: &Dom, node: NodeId, name: &[u8]) -> StrRef {
    dom.attr_ref(node, name).unwrap_or(StrRef::EMPTY)
}

fn dimension(raw: Option<&[u8]>) -> u16 {
    let Some(raw) = raw else { return 0 };
    let mut value: u32 = 0;
    let mut any = false;
    for &b in raw {
        if b.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((b - b'0') as u32);
            any = true;
        } else {
            break;
        }
    }
    if any {
        value.min(u16::MAX as u32) as u16
    } else {
        0
    }
}

fn apply_declarations(style: &mut BoxStyle, raw: Option<&[u8]>) {
    let Some(raw) = raw else { return };
    for decl in raw.split(|&b| b == b';') {
        let decl = trim_ascii(decl);
        if decl.is_empty() {
            continue;
        }
        let Some(colon) = decl.iter().position(|&b| b == b':') else {
            continue;
        };
        let prop = trim_ascii(&decl[..colon]);
        let val = trim_ascii(&decl[colon + 1..]);
        if prop.is_empty() || val.is_empty() {
            continue;
        }
        apply_single_declaration(style, prop, val);
    }
}

fn apply_single_declaration(style: &mut BoxStyle, prop: &[u8], val: &[u8]) {
    if prop.eq_ignore_ascii_case(b"text-align") {
        if val.eq_ignore_ascii_case(b"left") {
            style.text_align = TextAlign::Left;
        } else if val.eq_ignore_ascii_case(b"center") {
            style.text_align = TextAlign::Center;
        } else if val.eq_ignore_ascii_case(b"right") {
            style.text_align = TextAlign::Right;
        } else {
            style.record_unsupported(prop);
        }
    } else if prop.eq_ignore_ascii_case(b"list-style-type") || prop.eq_ignore_ascii_case(b"list-style") {
        if val.eq_ignore_ascii_case(b"disc") {
            style.list_style_type = ListStyleType::Disc;
        } else if val.eq_ignore_ascii_case(b"decimal") {
            style.list_style_type = ListStyleType::Decimal;
        } else if val.eq_ignore_ascii_case(b"none") {
            style.list_style_type = ListStyleType::None;
        } else {
            style.record_unsupported(prop);
        }
    } else if prop.eq_ignore_ascii_case(b"white-space") {
        if val.eq_ignore_ascii_case(b"normal") {
            style.white_space = WhiteSpace::Normal;
            style.preformatted = false;
        } else if val.eq_ignore_ascii_case(b"nowrap") {
            style.white_space = WhiteSpace::Nowrap;
            style.preformatted = false;
        } else if val.eq_ignore_ascii_case(b"pre") || val.eq_ignore_ascii_case(b"pre-wrap") {
            style.white_space = WhiteSpace::Pre;
            style.preformatted = true;
        } else {
            style.record_unsupported(prop);
        }
    } else if prop.eq_ignore_ascii_case(b"position") {
        if val.eq_ignore_ascii_case(b"static") {
            style.position = Position::Static;
        } else if val.eq_ignore_ascii_case(b"relative") {
            style.position = Position::Relative;
        } else if val.eq_ignore_ascii_case(b"absolute") {
            style.position = Position::Absolute;
        } else {
            style.record_unsupported(prop);
        }
    } else if prop.eq_ignore_ascii_case(b"top") {
        if let Some(n) = parse_css_length(val) {
            style.top = n;
            style.has_top = true;
        }
    } else if prop.eq_ignore_ascii_case(b"bottom") {
        if let Some(n) = parse_css_length(val) {
            style.bottom = n;
            style.has_bottom = true;
        }
    } else if prop.eq_ignore_ascii_case(b"left") {
        if let Some(n) = parse_css_length(val) {
            style.left = n;
            style.has_left = true;
        }
    } else if prop.eq_ignore_ascii_case(b"right") {
        if let Some(n) = parse_css_length(val) {
            style.right = n;
            style.has_right = true;
        }
    } else if is_known_css_property(prop) {
        // Standard core properties are cascaded by css::cascade or handled at the element level.
    } else {
        // Emit explicit unsupported notice instead of silently ignoring unknown styles.
        style.record_unsupported(prop);
    }
}

fn is_known_css_property(prop: &[u8]) -> bool {
    prop.eq_ignore_ascii_case(b"color")
        || prop.eq_ignore_ascii_case(b"background-color")
        || prop.eq_ignore_ascii_case(b"background")
        || prop.eq_ignore_ascii_case(b"font-size")
        || prop.eq_ignore_ascii_case(b"font-weight")
        || prop.eq_ignore_ascii_case(b"margin")
        || prop.eq_ignore_ascii_case(b"margin-top")
        || prop.eq_ignore_ascii_case(b"margin-bottom")
        || prop.eq_ignore_ascii_case(b"margin-left")
        || prop.eq_ignore_ascii_case(b"margin-right")
        || prop.eq_ignore_ascii_case(b"padding")
        || prop.eq_ignore_ascii_case(b"padding-top")
        || prop.eq_ignore_ascii_case(b"padding-bottom")
        || prop.eq_ignore_ascii_case(b"padding-left")
        || prop.eq_ignore_ascii_case(b"padding-right")
        || prop.eq_ignore_ascii_case(b"border")
        || prop.eq_ignore_ascii_case(b"border-color")
        || prop.eq_ignore_ascii_case(b"display")
        || prop.eq_ignore_ascii_case(b"width")
        || prop.eq_ignore_ascii_case(b"height")
}

fn parse_css_length(val: &[u8]) -> Option<i16> {
    let val = trim_ascii(val);
    if val.is_empty() {
        return None;
    }
    let (neg, val) = if val[0] == b'-' {
        (true, &val[1..])
    } else {
        (false, val)
    };
    let mut num: i32 = 0;
    let mut any = false;
    for &b in val {
        if b.is_ascii_digit() {
            num = num.saturating_mul(10).saturating_add((b - b'0') as i32);
            any = true;
        } else {
            break;
        }
    }
    if !any {
        return None;
    }
    let res = if neg { -num } else { num };
    Some(res.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = s.split_first() {
        if first == b' ' || first == b'\t' || first == b'\n' || first == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = s.split_last() {
        if last == b' ' || last == b'\t' || last == b'\n' || last == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn styled<'a>(dom: &'a Dom, html: &[u8], tag: &[u8]) -> BoxStyle {
        let _ = html;
        let node = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(dom) == tag)
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        compute(dom, node, &sheet, None)
    }

    #[test]
    fn a_heading_is_a_scaled_bold_block_with_margins() {
        let mut dom = Dom::new();
        dom.parse(b"<h1>Title</h1>");
        let style = styled(&dom, b"", b"h1");
        assert_eq!(style.display, Display::Block);
        assert_eq!(style.scale, 2);
        assert!(style.bold);
        assert!(style.margin_top > 0);
        assert!(style.margin_bottom > 0);
    }

    #[test]
    fn an_anchor_is_inline_and_a_list_item_is_not() {
        let mut dom = Dom::new();
        dom.parse(b"<ul><li><a href='/x'>x</a></li></ul>");
        let anchor = styled(&dom, b"", b"a");
        let item = styled(&dom, b"", b"li");
        assert_eq!(anchor.display, Display::Inline);
        assert_eq!(item.display, Display::ListItem);
        assert!(item.list_item);
        assert!(item.padding_left == 0);
    }

    #[test]
    fn inherited_colour_and_rung_reach_a_child() {
        let mut dom = Dom::new();
        dom.parse(b"<div style='color:#112233;font-size:24px'>a<span>b</span></div>");
        let div = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"div")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let span = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"span")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let parent = compute(&dom, div, &sheet, None);
        let child = compute(&dom, span, &sheet, Some(&parent));
        assert_eq!(child.css.color, 0xFF112233);
        assert!(child.css.has_color);
        assert_eq!(child.scale, 2);
    }

    #[test]
    fn a_hidden_element_is_not_laid_out() {
        let mut dom = Dom::new();
        dom.parse(b"<input type='hidden' name='csrf' value='x'><p>shown</p>");
        let input = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"input")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let style = compute(&dom, input, &sheet, None);
        assert_eq!(style.display, Display::None);
        assert!(style.hidden);
    }

    #[test]
    fn a_input_is_a_field_with_its_name_and_value() {
        let mut dom = Dom::new();
        dom.parse(b"<form><input name='q' value='rust' placeholder='Search'></form>");
        let input = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"input")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let style = compute(&dom, input, &sheet, None);
        assert_eq!(style.field, FieldKind::Input);
        assert_eq!(style.input_type, InputType::Text);
        assert_eq!(style.field_name.len as usize, 1);
        assert_eq!(style.field_value.len as usize, 4);
        assert!(style.height >= 40);
    }

    #[test]
    fn a_presentational_width_attribute_is_read() {
        let mut dom = Dom::new();
        dom.parse(b"<img src='/a.png' width='320' height='180'>");
        let img = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"img")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let style = compute(&dom, img, &sheet, None);
        assert_eq!(style.width, 320);
        assert_eq!(style.height, 180);
    }

    #[test]
    fn text_align_properties_are_parsed_and_inherited() {
        let mut dom = Dom::new();
        dom.parse(b"<div style='text-align: center;'><p style='text-align: right;'>right</p><span>inherit</span></div>");
        let div = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"div")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let p = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"p")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let span = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"span")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let div_style = compute(&dom, div, &sheet, None);
        let p_style = compute(&dom, p, &sheet, Some(&div_style));
        let span_style = compute(&dom, span, &sheet, Some(&div_style));

        assert_eq!(div_style.text_align, TextAlign::Center);
        assert_eq!(p_style.text_align, TextAlign::Right);
        assert_eq!(span_style.text_align, TextAlign::Center);
    }

    #[test]
    fn list_style_type_defaults_and_override_none() {
        let mut dom = Dom::new();
        dom.parse(b"<ul><li>default</li><li style='list-style-type: none;'>none</li><li style='list-style-type: decimal;'>num</li></ul>");
        let lis: std::vec::Vec<NodeId> = dom
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_element() && n.tag.get(&dom) == b"li")
            .map(|(i, _)| i as NodeId)
            .collect();
        let sheet = css::Stylesheet::empty();
        let li0 = compute(&dom, lis[0], &sheet, None);
        let li1 = compute(&dom, lis[1], &sheet, None);
        let li2 = compute(&dom, lis[2], &sheet, None);

        assert_eq!(li0.list_style_type, ListStyleType::Disc);
        assert_eq!(li1.list_style_type, ListStyleType::None);
        assert_eq!(li2.list_style_type, ListStyleType::Decimal);
    }

    #[test]
    fn white_space_and_positioning_properties() {
        let mut dom = Dom::new();
        dom.parse(b"<div style='white-space: nowrap; position: relative; top: 12px; left: 24px;'><p style='white-space: pre; position: absolute;'>code</p></div>");
        let div = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"div")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let p = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"p")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let div_style = compute(&dom, div, &sheet, None);
        let p_style = compute(&dom, p, &sheet, Some(&div_style));

        assert_eq!(div_style.white_space, WhiteSpace::Nowrap);
        assert_eq!(div_style.position, Position::Relative);
        assert_eq!(div_style.top, 12);
        assert!(div_style.has_top);
        assert_eq!(div_style.left, 24);
        assert!(div_style.has_left);

        assert_eq!(p_style.white_space, WhiteSpace::Pre);
        assert!(p_style.preformatted);
        assert_eq!(p_style.position, Position::Absolute);
    }

    #[test]
    fn unsupported_styles_emit_explicit_notices() {
        let mut dom = Dom::new();
        dom.parse(b"<div style='transform: rotate(45deg); opacity: 0.8; color: #ff0000;'>test</div>");
        let div = dom
            .nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(&dom) == b"div")
            .map(|(i, _)| i as NodeId)
            .unwrap();
        let sheet = css::Stylesheet::empty();
        let style = compute(&dom, div, &sheet, None);

        assert!(style.has_unsupported());
        assert_eq!(style.unsupported_count, 2);
        assert_eq!(style.unsupported[0].as_str(), "transform");
        assert_eq!(style.unsupported[1].as_str(), "opacity");
        // Supported properties are still applied
        assert_eq!(style.css.color, 0xFFFF0000);
    }
}
