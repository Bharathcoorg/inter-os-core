//! A bounded DOM for the browser's layout engine.
//!
//! WHY this exists: the reader flattens a page into lines before it is
//! drawn, which is why it cannot express a table, a form control or a
//! block whose width depends on its parent. A browser view needs the
//! element structure to lay the page out, so this module keeps a small
//! tree of elements and text.
//!
//! Everything is fixed-capacity and zero-allocation. Nodes, attributes,
//! decoded text and the first stylesheet live in arrays owned by [`Dom`].
//! A document that exceeds a capacity is parsed up to that capacity and
//! reports [`Dom::truncated`]; nothing panics and nothing is invented.
//!
//! The parser is deliberately forgiving in the way browsers are: void
//! elements never wait for a close tag, a paragraph is closed by the next
//! block element, a list item by the next item, and a close tag only pops
//! when it matches an open element.

/// The most nodes one document may hold.
pub const MAX_NODES: usize = 3072;

/// The most decoded text bytes one document may hold.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// The most attributes one document may hold.
pub const MAX_ATTRS: usize = 4096;

/// The deepest element nesting the parser tracks.
///
/// WHY a parser is bounded but the tree is not: an element opened past
/// this depth is still added as a child, it just does not become the
/// parent of what follows. A page with a runaway <div> therefore keeps
/// its text instead of losing it.
pub const MAX_DEPTH: usize = 64;

/// The most links (anchors with an href) one document may hold.
pub const MAX_LINKS: usize = 128;

/// The most bytes of the first inline stylesheet that are kept.
pub const MAX_CSS_BYTES: usize = 16 * 1024;

/// The longest tag name kept whole. A longer name is truncated, which can
/// only make it stop matching a selector; it never splits a node.
const MAX_TAG_BYTES: usize = 20;

/// An index into a [`Dom`]'s arenas.
pub type NodeId = u16;

/// The absence of a node.
pub const NO_NODE: NodeId = u16::MAX;

/// The absence of a link.
pub const NO_LINK: u8 = u8::MAX;

/// A range in the document's text pool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StrRef {
    pub start: u32,
    pub len: u16,
}

impl StrRef {
    pub const EMPTY: Self = Self { start: 0, len: 0 };

    pub fn from_slice(bytes: &[u8], start: usize) -> Self {
        Self {
            start: start as u32,
            len: bytes.len().min(u16::MAX as usize) as u16,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get<'a>(&self, dom: &'a Dom) -> &'a [u8] {
        let start = (self.start as usize).min(dom.text_len);
        let end = (start + self.len as usize).min(dom.text_len);
        &dom.text[start..end]
    }
}

/// What a node is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Document,
    Element,
    Text,
}

/// One attribute, with both strings in the text pool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attr {
    pub name: StrRef,
    pub value: StrRef,
}

impl Attr {
    pub const EMPTY: Self = Self {
        name: StrRef::EMPTY,
        value: StrRef::EMPTY,
    };
}

/// One node of the tree.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub kind: NodeKind,
    /// The lowercased tag name for an element, empty otherwise.
    pub tag: StrRef,
    /// The decoded text for a text node, empty otherwise.
    pub text: StrRef,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub last_child: NodeId,
    pub next_sibling: NodeId,
    pub attr_start: u16,
    pub attr_count: u16,
    /// The anchor this element opens, or [`NO_LINK`].
    pub link: u8,
}

impl Node {
    pub const EMPTY: Self = Self {
        kind: NodeKind::Document,
        tag: StrRef::EMPTY,
        text: StrRef::EMPTY,
        parent: NO_NODE,
        first_child: NO_NODE,
        last_child: NO_NODE,
        next_sibling: NO_NODE,
        attr_start: 0,
        attr_count: 0,
        link: NO_LINK,
    };

    pub fn is_element(&self) -> bool {
        self.kind == NodeKind::Element
    }

    pub fn is_text(&self) -> bool {
        self.kind == NodeKind::Text
    }
}

/// The bounded document tree.
pub struct Dom {
    pub(super) nodes: [Node; MAX_NODES],
    pub(super) node_count: usize,
    pub(super) text: [u8; MAX_TEXT_BYTES],
    pub(super) text_len: usize,
    pub(super) attrs: [Attr; MAX_ATTRS],
    pub(super) attr_count: usize,
    links: [StrRef; MAX_LINKS],
    link_count: usize,
    css: [u8; MAX_CSS_BYTES],
    css_len: usize,
    root: NodeId,
    truncated: bool,
}

impl Dom {
    pub const fn new() -> Self {
        Self {
            nodes: [Node::EMPTY; MAX_NODES],
            node_count: 0,
            text: [0; MAX_TEXT_BYTES],
            text_len: 0,
            attrs: [Attr::EMPTY; MAX_ATTRS],
            attr_count: 0,
            links: [StrRef::EMPTY; MAX_LINKS],
            link_count: 0,
            css: [0; MAX_CSS_BYTES],
            css_len: 0,
            root: NO_NODE,
            truncated: false,
        }
    }

    /// Parses `html` and returns the document node.
    pub fn parse(&mut self, html: &[u8]) -> NodeId {
        self.reset();
        let root = self.push_node(NodeKind::Document, StrRef::EMPTY, StrRef::EMPTY);
        self.root = root;
        let mut stack = [NO_NODE; MAX_DEPTH];
        stack[0] = root;
        let mut depth = 1usize;
        let mut i = 0usize;
        let n = html.len();
        let mut css_seen = false;

        while i < n {
            if html[i] != b'<' {
                let start = i;
                while i < n && html[i] != b'<' {
                    i += 1;
                }
                let parent = stack[depth - 1];
                self.push_text(parent, &html[start..i]);
                continue;
            }

            if html[i..].starts_with(b"<!--") {
                i = find_after(html, i + 4, b"-->").unwrap_or(n);
                continue;
            }
            if html[i..].starts_with(b"<!") || html[i..].starts_with(b"<?") {
                i = find_after(html, i + 2, b">").unwrap_or(n);
                continue;
            }

            if html[i..].starts_with(b"</") {
                let (name, name_len, next) = parse_close_tag(html, i + 2);
                i = next;
                if name_len == 0 {
                    continue;
                }
                close_element(&mut stack, &mut depth, &name[..name_len], self);
                continue;
            }

            let Some((tag, next)) = parse_open_tag(html, i + 1) else {
                // A lone `<` is text, which is what a browser shows too.
                self.push_text(stack[depth - 1], b"<");
                i += 1;
                continue;
            };
            i = next;

            // Raw-text elements are not markup. A script body is skipped
            // whole; the first stylesheet is kept because the layout
            // needs it; a textarea's body is text.
            if tag.name_eq(b"script") {
                if tag.has_attribute(b"src") || tag.self_closing {
                    continue;
                }
                i = find_close(html, i, b"script").unwrap_or(n);
                continue;
            }
            if tag.name_eq(b"style") {
                let start = i;
                if tag.self_closing {
                    continue;
                }
                i = find_close(html, i, b"style").unwrap_or(n);
                if !css_seen {
                    let end = find_close_start(html, start, b"style").unwrap_or(i);
                    self.capture_css(&html[start..end]);
                    css_seen = true;
                }
                continue;
            }
            if tag.name_eq(b"title") {
                if tag.self_closing {
                    continue;
                }
                i = find_close(html, i, b"title").unwrap_or(n);
                continue;
            }
            if tag.name_eq(b"textarea") {
                let start = i;
                i = find_close(html, i, b"textarea").unwrap_or(n);
                let end = find_close_start(html, start, b"textarea").unwrap_or(i);
                let node = self.open_element(&tag, &mut stack, &mut depth);
                if node != NO_NODE {
                    self.push_text(node, &html[start..end]);
                }
                continue;
            }

            let self_closing = tag.self_closing || is_void(tag.name());
            let node = self.open_element(&tag, &mut stack, &mut depth);
            if node == NO_NODE {
                continue;
            }
            if !self_closing && depth < MAX_DEPTH {
                stack[depth] = node;
                depth += 1;
            } else if !self_closing {
                // Deeper than the parser tracks: the element exists, but
                // what follows belongs to the nearest tracked ancestor.
                self.truncated = true;
            }
        }

        self.root
    }

    fn reset(&mut self) {
        self.node_count = 0;
        self.text_len = 0;
        self.attr_count = 0;
        self.link_count = 0;
        self.css_len = 0;
        self.root = NO_NODE;
        self.truncated = false;
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[(id as usize).min(MAX_NODES - 1)]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes[..self.node_count]
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn tag<'a>(&'a self, id: NodeId) -> &'a [u8] {
        self.node(id).tag.get(self)
    }

    pub fn text_of<'a>(&'a self, id: NodeId) -> &'a [u8] {
        self.node(id).text.get(self)
    }

    /// The value of `name` on this element, or `None`.
    pub fn attr<'a>(&'a self, id: NodeId, name: &[u8]) -> Option<&'a [u8]> {
        let node = self.node(id);
        let start = node.attr_start as usize;
        let end = (start + node.attr_count as usize).min(self.attr_count);
        for attr in &self.attrs[start.min(end)..end] {
            if attr.name.get(self).eq_ignore_ascii_case(name) {
                return Some(attr.value.get(self));
            }
        }
        None
    }

    pub fn has_attr(&self, id: NodeId, name: &[u8]) -> bool {
        self.attr(id, name).is_some()
    }

    /// The stored range of an attribute value, for a caller that keeps a
    /// [`StrRef`] rather than borrowing the DOM.
    pub fn attr_ref(&self, id: NodeId, name: &[u8]) -> Option<StrRef> {
        let node = self.node(id);
        let start = node.attr_start as usize;
        let end = (start + node.attr_count as usize).min(self.attr_count);
        for attr in &self.attrs[start.min(end)..end] {
            if attr.name.get(self).eq_ignore_ascii_case(name) {
                return Some(attr.value);
            }
        }
        None
    }

    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        let child = self.node(id).first_child;
        if child == NO_NODE {
            None
        } else {
            Some(child)
        }
    }

    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let next = self.node(id).next_sibling;
        if next == NO_NODE {
            None
        } else {
            Some(next)
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.node(id).parent;
        if parent == NO_NODE {
            None
        } else {
            Some(parent)
        }
    }

    pub fn children<'a>(&'a self, id: NodeId) -> Children<'a> {
        Children {
            dom: self,
            next: self.node(id).first_child,
        }
    }

    /// The `href` of a link id, or `None` when the anchor had no value.
    pub fn link_href(&self, link: u8) -> Option<&[u8]> {
        if link == NO_LINK || link as usize >= self.link_count {
            return None;
        }
        let href = self.links[link as usize].get(self);
        if href.is_empty() {
            None
        } else {
            Some(href)
        }
    }

    /// The first inline stylesheet, as written.
    pub fn css(&self) -> &[u8] {
        &self.css[..self.css_len]
    }

    /// Whether any capacity was reached while parsing.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    fn capture_css(&mut self, css: &[u8]) {
        let n = css.len().min(MAX_CSS_BYTES);
        self.css[..n].copy_from_slice(&css[..n]);
        self.css_len = n;
        if css.len() > n {
            self.truncated = true;
        }
    }

    fn push_node(&mut self, kind: NodeKind, tag: StrRef, text: StrRef) -> NodeId {
        if self.node_count >= MAX_NODES {
            self.truncated = true;
            return NO_NODE;
        }
        let id = self.node_count as NodeId;
        self.nodes[self.node_count] = Node {
            kind,
            tag,
            text,
            parent: NO_NODE,
            first_child: NO_NODE,
            last_child: NO_NODE,
            next_sibling: NO_NODE,
            attr_start: self.attr_count as u16,
            attr_count: 0,
            link: NO_LINK,
        };
        self.node_count += 1;
        id
    }

    fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if parent == NO_NODE || child == NO_NODE {
            return;
        }
        self.nodes[child as usize].parent = parent;
        let last = self.nodes[parent as usize].last_child;
        if last == NO_NODE {
            self.nodes[parent as usize].first_child = child;
        } else {
            self.nodes[last as usize].next_sibling = child;
        }
        self.nodes[parent as usize].last_child = child;
    }

    fn intern(&mut self, bytes: &[u8]) -> StrRef {
        if bytes.is_empty() {
            return StrRef::EMPTY;
        }
        let room = MAX_TEXT_BYTES - self.text_len;
        let n = bytes.len().min(room).min(u16::MAX as usize);
        if n < bytes.len() {
            self.truncated = true;
        }
        if n == 0 {
            return StrRef::EMPTY;
        }
        let start = self.text_len;
        self.text[start..start + n].copy_from_slice(&bytes[..n]);
        self.text_len += n;
        StrRef {
            start: start as u32,
            len: n as u16,
        }
    }

    fn push_text(&mut self, parent: NodeId, raw: &[u8]) {
        if parent == NO_NODE || raw.is_empty() {
            return;
        }
        let start = self.text_len;
        self.decode_entities_into(raw);
        if self.text_len == start {
            return;
        }
        let len = (self.text_len - start).min(u16::MAX as usize) as u16;
        let last = self.node(parent).last_child;
        if last != NO_NODE {
            let node = &mut self.nodes[last as usize];
            if node.is_text() {
                let end = node.text.start as usize + node.text.len as usize;
                if end == start {
                    node.text.len = node.text.len.saturating_add(len);
                    return;
                }
            }
        }
        let text = StrRef {
            start: start as u32,
            len,
        };
        let id = self.push_node(NodeKind::Text, StrRef::EMPTY, text);
        self.append_child(parent, id);
    }

    fn open_element(&mut self, tag: &OpenTag<'_>, stack: &mut [NodeId; MAX_DEPTH], depth: &mut usize) -> NodeId {
        // A block element closes an open paragraph. This is what makes
        // `<p>one<div>two</div>three` lay out as two paragraphs rather
        // than one paragraph containing a div.
        if is_block_name(tag.name()) {
            while *depth > 1 {
                let top = stack[*depth - 1];
                if self.tag(top).eq_ignore_ascii_case(b"p") {
                    *depth -= 1;
                } else {
                    break;
                }
            }
        }
        if tag.name_eq(b"li") {
            self.close_while(stack, depth, b"li");
        } else if tag.name_eq(b"tr") {
            self.close_while(stack, depth, b"td");
            self.close_while(stack, depth, b"th");
            self.close_while(stack, depth, b"li");
        } else if tag.name_eq(b"td") || tag.name_eq(b"th") {
            self.close_while(stack, depth, b"td");
            self.close_while(stack, depth, b"th");
        } else if tag.name_eq(b"option") {
            self.close_while(stack, depth, b"option");
        } else if tag.name_eq(b"dt") || tag.name_eq(b"dd") {
            self.close_while(stack, depth, b"dt");
            self.close_while(stack, depth, b"dd");
        } else if tag.name_eq(b"p") {
            self.close_while(stack, depth, b"p");
        } else if is_heading(tag.name()) {
            self.close_open_heading(stack, depth);
        } else if tag.name_eq(b"a") {
            // Anchors do not nest. A new one closes the previous one.
            self.close_while(stack, depth, b"a");
        }

        let tag_ref = self.intern(tag.name());
        let id = self.push_node(NodeKind::Element, tag_ref, StrRef::EMPTY);
        if id == NO_NODE {
            return NO_NODE;
        }
        let attr_start = self.attr_count;
        let mut parser = tag.attrs();
        while let Some(attr) = parser.next() {
            if !is_layout_attribute(attr.name()) {
                continue;
            }
            if self.attr_count >= MAX_ATTRS {
                self.truncated = true;
                break;
            }
            let name = self.intern(attr.name());
            let value = self.intern_decode(&attr.value);
            self.attrs[self.attr_count] = Attr { name, value };
            self.attr_count += 1;
        }
        let count = self.attr_count - attr_start;
        self.nodes[id as usize].attr_start = attr_start as u16;
        self.nodes[id as usize].attr_count = count as u16;

        if tag.name_eq(b"a") {
            let mut href = [0u8; 160];
            let mut href_len = 0usize;
            let start = self.nodes[id as usize].attr_start as usize;
            let end = start + self.nodes[id as usize].attr_count as usize;
            for attr in &self.attrs[start..end.min(self.attr_count)] {
                if attr.name.get(self) == b"href" {
                    let value = attr.value.get(self);
                    href_len = value.len().min(href.len());
                    href[..href_len].copy_from_slice(&value[..href_len]);
                    break;
                }
            }
            if href_len > 0 && self.link_count < MAX_LINKS {
                let link = self.link_count;
                self.links[link] = self.intern(&href[..href_len]);
                self.link_count += 1;
                self.nodes[id as usize].link = link as u8;
            }
        }
        let parent = stack[*depth - 1];
        self.append_child(parent, id);
        id
    }

    fn close_while(&mut self, stack: &mut [NodeId; MAX_DEPTH], depth: &mut usize, name: &[u8]) {
        while *depth > 1 {
            let top = stack[*depth - 1];
            if self.tag(top).eq_ignore_ascii_case(name) {
                *depth -= 1;
            } else {
                break;
            }
        }
    }

    fn close_open_heading(&mut self, stack: &mut [NodeId; MAX_DEPTH], depth: &mut usize) {
        while *depth > 1 {
            let top = stack[*depth - 1];
            let tag = self.tag(top);
            if is_heading(tag) {
                *depth -= 1;
            } else {
                break;
            }
        }
    }

    fn intern_decode(&mut self, raw: &[u8]) -> StrRef {
        let start = self.text_len;
        self.decode_entities_into(raw);
        StrRef {
            start: start as u32,
            len: (self.text_len - start) as u16,
        }
    }

    /// Decodes the character references a page uses and appends them.
    fn decode_entities_into(&mut self, raw: &[u8]) {
        let mut i = 0usize;
        while i < raw.len() {
            if raw[i] == b'&' {
                if let Some((decoded, consumed)) = decode_entity(&raw[i..]) {
                    self.push_char(decoded);
                    i += consumed;
                    continue;
                }
            }
            if self.text_len < MAX_TEXT_BYTES {
                self.text[self.text_len] = raw[i];
                self.text_len += 1;
            } else {
                self.truncated = true;
                return;
            }
            i += 1;
        }
    }

    fn push_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let bytes = ch.encode_utf8(&mut buf).as_bytes();
        for &b in bytes {
            if self.text_len < MAX_TEXT_BYTES {
                self.text[self.text_len] = b;
                self.text_len += 1;
            } else {
                self.truncated = true;
                return;
            }
        }
    }
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterates a node's children.
pub struct Children<'a> {
    dom: &'a Dom,
    next: NodeId,
}

impl<'a> Iterator for Children<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next;
        if id == NO_NODE {
            return None;
        }
        self.next = self.dom.node(id).next_sibling;
        Some(id)
    }
}

/// Parsed open tag data. The name is lowercased in place.
struct OpenTag<'a> {
    name: [u8; MAX_TAG_BYTES],
    name_len: usize,
    rest: &'a [u8],
    self_closing: bool,
}

impl<'a> OpenTag<'a> {
    fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }

    fn name_eq(&self, other: &[u8]) -> bool {
        self.name().eq_ignore_ascii_case(other)
    }

    fn attrs(&self) -> AttrParser<'a> {
        AttrParser { rest: self.rest }
    }

    fn has_attribute(&self, name: &[u8]) -> bool {
        let mut attrs = self.attrs();
        while let Some(attr) = attrs.next() {
            if attr.name().eq_ignore_ascii_case(name) {
                return true;
            }
        }
        false
    }
}

/// Parses `<name ...>` starting after the `<`.
fn parse_open_tag(html: &[u8], start: usize) -> Option<(OpenTag<'_>, usize)> {
    let mut i = start;
    let n = html.len();
    while i < n && is_space(html[i]) {
        i += 1;
    }
    if i >= n || !is_name_start(html[i]) {
        return None;
    }
    let mut name = [0u8; MAX_TAG_BYTES];
    let mut name_len = 0usize;
    while i < n && is_name_char(html[i]) {
        if name_len < MAX_TAG_BYTES {
            name[name_len] = html[i].to_ascii_lowercase();
            name_len += 1;
        }
        i += 1;
    }
    let attr_start = i;
    let mut quote: Option<u8> = None;
    while i < n {
        let b = html[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => {
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'>' {
                    break;
                }
            }
        }
        i += 1;
    }
    if i >= n {
        return None;
    }
    let rest = &html[attr_start..i];
    let self_closing = rest
        .iter()
        .rposition(|&b| !is_space(b))
        .map(|p| rest[p] == b'/')
        .unwrap_or(false);
    Some((
        OpenTag {
            name,
            name_len,
            rest,
            self_closing,
        },
        i + 1,
    ))
}

/// Parses `</name>` starting after `</`. Returns the lowercased name, its
/// length, and the offset after the tag.
fn parse_close_tag(html: &[u8], start: usize) -> ([u8; MAX_TAG_BYTES], usize, usize) {
    let mut name = [0u8; MAX_TAG_BYTES];
    let mut len = 0usize;
    let mut i = start;
    let n = html.len();
    while i < n && is_space(html[i]) {
        i += 1;
    }
    while i < n && is_name_char(html[i]) {
        if len < MAX_TAG_BYTES {
            name[len] = html[i].to_ascii_lowercase();
            len += 1;
        }
        i += 1;
    }
    while i < n && html[i] != b'>' {
        i += 1;
    }
    if i < n {
        i += 1;
    }
    (name, len, i)
}

struct AttrParser<'a> {
    rest: &'a [u8],
}

struct ParsedAttr<'a> {
    name: [u8; MAX_TAG_BYTES],
    name_len: usize,
    value: &'a [u8],
}

impl<'a> AttrParser<'a> {
    fn next(&mut self) -> Option<ParsedAttr<'a>> {
        let bytes = self.rest;
        let mut i = 0usize;
        while i < bytes.len() && (is_space(bytes[i]) || bytes[i] == b'/') {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let mut name = [0u8; MAX_TAG_BYTES];
        let mut name_len = 0usize;
        while i < bytes.len() && !is_space(bytes[i]) && bytes[i] != b'=' && bytes[i] != b'/' {
            if name_len < MAX_TAG_BYTES {
                name[name_len] = bytes[i].to_ascii_lowercase();
                name_len += 1;
            }
            i += 1;
        }
        while i < bytes.len() && is_space(bytes[i]) {
            i += 1;
        }
        let mut value = &b""[..];
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && is_space(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                value = &bytes[start..i];
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                let start = i;
                while i < bytes.len() && !is_space(bytes[i]) {
                    i += 1;
                }
                value = &bytes[start..i];
            }
        }
        self.rest = &bytes[i..];
        if name_len == 0 {
            return self.next();
        }
        Some(ParsedAttr {
            name,
            name_len,
            value,
        })
    }
}

impl<'a> ParsedAttr<'a> {
    fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c')
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'!' || b == b'?'
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.')
}

fn find_after(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i + needle.len());
        }
        i += 1;
    }
    None
}

fn find_close_start(haystack: &[u8], from: usize, tag: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + 2 + tag.len() <= haystack.len() {
        if haystack[i] == b'<' && haystack[i + 1] == b'/' {
            let rest = &haystack[i + 2..];
            if rest.len() >= tag.len() && rest[..tag.len()].eq_ignore_ascii_case(tag) {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn find_close(haystack: &[u8], from: usize, tag: &[u8]) -> Option<usize> {
    let start = find_close_start(haystack, from, tag)?;
    let mut i = start + 2 + tag.len();
    while i < haystack.len() && haystack[i] != b'>' {
        i += 1;
    }
    Some(if i < haystack.len() { i + 1 } else { haystack.len() })
}

fn close_element(
    stack: &mut [NodeId; MAX_DEPTH],
    depth: &mut usize,
    name: &[u8],
    dom: &Dom,
) {
    let mut index = None;
    let mut i = *depth;
    while i > 1 {
        i -= 1;
        if dom.tag(stack[i]).eq_ignore_ascii_case(name) {
            index = Some(i);
            break;
        }
    }
    if let Some(i) = index {
        *depth = i;
    }
}

fn is_void(name: &[u8]) -> bool {
    matches!(
        name,
        b"area"
            | b"base"
            | b"br"
            | b"col"
            | b"embed"
            | b"hr"
            | b"img"
            | b"input"
            | b"link"
            | b"meta"
            | b"param"
            | b"source"
            | b"track"
            | b"wbr"
    )
}

fn is_heading(name: &[u8]) -> bool {
    matches!(name, b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6")
}

pub(super) fn is_block_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"address"
            | b"article"
            | b"aside"
            | b"blockquote"
            | b"body"
            | b"dd"
            | b"div"
            | b"dl"
            | b"dt"
            | b"fieldset"
            | b"figcaption"
            | b"figure"
            | b"footer"
            | b"form"
            | b"h1"
            | b"h2"
            | b"h3"
            | b"h4"
            | b"h5"
            | b"h6"
            | b"header"
            | b"hr"
            | b"li"
            | b"main"
            | b"nav"
            | b"ol"
            | b"p"
            | b"pre"
            | b"section"
            | b"table"
            | b"tbody"
            | b"td"
            | b"tfoot"
            | b"th"
            | b"thead"
            | b"tr"
            | b"ul"
    )
}

/// Only the attributes the layout engine consumes are kept. Keeping every
/// attribute of a framework-heavy page would spend the attribute arena on
/// data the renderer never reads.
fn is_layout_attribute(name: &[u8]) -> bool {
    matches!(
        name,
        b"class"
            | b"id"
            | b"style"
            | b"href"
            | b"src"
            | b"alt"
            | b"type"
            | b"name"
            | b"value"
            | b"placeholder"
            | b"action"
            | b"method"
            | b"width"
            | b"height"
            | b"colspan"
            | b"rowspan"
            | b"title"
            | b"label"
            | b"checked"
            | b"selected"
            | b"disabled"
            | b"hidden"
            | b"size"
            | b"maxlength"
    )
}

/// Decodes one character reference at the start of `raw` (which begins
/// with `&`). Returns the character and the bytes consumed, or `None`
/// when this ampersand is not a reference.
fn decode_entity(raw: &[u8]) -> Option<(char, usize)> {
    if raw.len() < 3 || raw[0] != b'&' {
        return None;
    }
    // Numeric: &#123; or &#x1F600;
    if raw[1] == b'#' {
        let (hex, digit_start) = if raw.get(2).map_or(false, |&b| b == b'x' || b == b'X') {
            (true, 3)
        } else {
            (false, 2)
        };
        let mut value: u32 = 0;
        let mut i = digit_start;
        let mut digits = 0usize;
        while i < raw.len() && i < 12 {
            let b = raw[i];
            let digit = if hex {
                match b {
                    b'0'..=b'9' => Some((b - b'0') as u32),
                    b'a'..=b'f' => Some((b - b'a' + 10) as u32),
                    b'A'..=b'F' => Some((b - b'A' + 10) as u32),
                    _ => None,
                }
            } else if b.is_ascii_digit() {
                Some((b - b'0') as u32)
            } else {
                None
            };
            let Some(d) = digit else { break };
            value = value.saturating_mul(if hex { 16 } else { 10 }).saturating_add(d);
            digits += 1;
            i += 1;
        }
        if digits == 0 || i >= raw.len() || raw[i] != b';' {
            return None;
        }
        let ch = char::from_u32(value)?;
        if ch == '\0' {
            return None;
        }
        return Some((ch, i + 1));
    }
    let semi = raw.iter().take(10).position(|&b| b == b';')?;
    let name = &raw[1..semi];
    let ch = match name {
        b"amp" => '&',
        b"lt" => '<',
        b"gt" => '>',
        b"quot" => '"',
        b"apos" => '\'',
        b"nbsp" => '\u{00A0}',
        b"copy" => '\u{00A9}',
        b"reg" => '\u{00AE}',
        b"trade" => '\u{2122}',
        b"hellip" => '\u{2026}',
        b"mdash" => '\u{2014}',
        b"ndash" => '\u{2013}',
        b"lsquo" => '\u{2018}',
        b"rsquo" => '\u{2019}',
        b"ldquo" => '\u{201C}',
        b"rdquo" => '\u{201D}',
        b"middot" => '\u{00B7}',
        b"bull" => '\u{2022}',
        b"deg" => '\u{00B0}',
        b"plusmn" => '\u{00B1}',
        b"times" => '\u{00D7}',
        b"divide" => '\u{00F7}',
        b"laquo" => '\u{00AB}',
        b"raquo" => '\u{00BB}',
        b"euro" => '\u{20AC}',
        b"pound" => '\u{00A3}',
        b"yen" => '\u{00A5}',
        b"cent" => '\u{00A2}',
        b"sect" => '\u{00A7}',
        b"para" => '\u{00B6}',
        _ => return None,
    };
    Some((ch, semi + 1))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn find(dom: &Dom, tag: &[u8]) -> Option<NodeId> {
        dom.nodes()
            .iter()
            .enumerate()
            .find(|(_, n)| n.is_element() && n.tag.get(dom).eq_ignore_ascii_case(tag))
            .map(|(i, _)| i as NodeId)
    }

    fn collect_text(dom: &Dom, id: NodeId, out: &mut std::vec::Vec<u8>) {
        let node = dom.node(id);
        if node.is_text() {
            out.extend_from_slice(node.text.get(dom));
            return;
        }
        for child in dom.children(id) {
            collect_text(dom, child, out);
        }
    }

    #[test]
    fn elements_nest_and_text_is_decoded() {
        let mut dom = Dom::new();
        let root = dom.parse(b"<html><body><p>One &amp; two</p></body></html>");
        assert_eq!(root, dom.root());
        let p = find(&dom, b"p").unwrap();
        assert_eq!(dom.tag(p), b"p");
        let mut text = std::vec::Vec::new();
        collect_text(&dom, p, &mut text);
        assert_eq!(text, b"One & two");
    }

    #[test]
    fn a_block_closes_an_open_paragraph() {
        let mut dom = Dom::new();
        dom.parse(b"<p>alpha<div>beta</div>gamma");
        let p = find(&dom, b"p").unwrap();
        let div = find(&dom, b"div").unwrap();
        assert_eq!(dom.node(div).parent, dom.node(p).parent);
        let mut text = std::vec::Vec::new();
        collect_text(&dom, div, &mut text);
        assert_eq!(text, b"beta");
    }

    #[test]
    fn void_elements_do_not_capture_following_text() {
        let mut dom = Dom::new();
        dom.parse(b"<div>a<br>b<img src='/x.png' alt='x'>c</div>");
        let div = find(&dom, b"div").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, div, &mut text);
        assert_eq!(text, b"abc");
        let img = find(&dom, b"img").unwrap();
        assert_eq!(dom.attr(img, b"src"), Some(&b"/x.png"[..]));
        assert_eq!(dom.attr(img, b"alt"), Some(&b"x"[..]));
    }

    #[test]
    fn a_list_item_is_closed_by_the_next_item() {
        let mut dom = Dom::new();
        dom.parse(b"<ul><li>one<li>two</ul>");
        let items: std::vec::Vec<_> = dom
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_element() && n.tag.get(&dom) == b"li")
            .map(|(i, _)| i as NodeId)
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(dom.node(items[0]).parent, dom.node(items[1]).parent);
    }

    #[test]
    fn the_first_stylesheet_is_kept_and_a_script_is_not_text() {
        let mut dom = Dom::new();
        dom.parse(b"<style>p{color:#112233}</style><p>ok</p><script>var x='<p>no</p>';</script>");
        assert_eq!(dom.css(), b"p{color:#112233}");
        let p = find(&dom, b"p").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, p, &mut text);
        assert_eq!(text, b"ok");
        assert!(find(&dom, b"script").is_none());
    }

    #[test]
    fn a_textarea_body_is_text_with_markup_inside_it() {
        let mut dom = Dom::new();
        dom.parse(b"<textarea><b>not bold</b></textarea>");
        let area = find(&dom, b"textarea").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, area, &mut text);
        assert_eq!(text, b"<b>not bold</b>");
    }

    #[test]
    fn a_mismatched_close_tag_is_ignored() {
        let mut dom = Dom::new();
        dom.parse(b"<div><p>kept</p></span></div>tail");
        let div = find(&dom, b"div").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, div, &mut text);
        assert_eq!(text, b"kept");
        let body = dom.node(div).parent;
        let mut all = std::vec::Vec::new();
        collect_text(&dom, body, &mut all);
        assert_eq!(all, b"kepttail");
    }

    #[test]
    fn an_anchor_keeps_its_href_and_link_id() {
        let mut dom = Dom::new();
        dom.parse(b"<p><a href='/docs?q=1&amp;r=2'>Docs</a></p>");
        let a = find(&dom, b"a").unwrap();
        assert_ne!(dom.node(a).link, NO_LINK);
        assert_eq!(dom.link_href(dom.node(a).link), Some(&b"/docs?q=1&r=2"[..]));
    }

    #[test]
    fn named_and_numeric_entities_decode() {
        let mut dom = Dom::new();
        dom.parse(b"<p>&lt;x&gt; &#65; &#x42; &nbsp; &unknown;</p>");
        let p = find(&dom, b"p").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, p, &mut text);
        assert_eq!(text, b"<x> A B \xc2\xa0 &unknown;");
    }

    #[test]
    fn attributes_outside_the_layout_set_are_dropped() {
        let mut dom = Dom::new();
        dom.parse(b"<div data-tracker='1' class='hero' style='color:red'>x</div>");
        let div = find(&dom, b"div").unwrap();
        assert_eq!(dom.attr(div, b"class"), Some(&b"hero"[..]));
        assert_eq!(dom.attr(div, b"data-tracker"), None);
    }

    #[test]
    fn a_document_beyond_the_node_budget_is_reported_truncated() {
        let mut dom = Dom::new();
        let mut html = std::vec::Vec::new();
        for _ in 0..(MAX_NODES + 8) {
            html.extend_from_slice(b"<i>x</i>");
        }
        dom.parse(&html);
        assert!(dom.truncated());
        assert_eq!(dom.node_count(), MAX_NODES);
    }
}
