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
    };

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
        // CSS white-space inherits.
        style.preformatted |= parent.preformatted;
    }

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
}
