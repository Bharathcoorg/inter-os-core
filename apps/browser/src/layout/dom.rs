//! A bounded DOM for the browser's layout engine.
//!
//! WHY this exists: the reader flattens a page into lines before it is
//! drawn, which is why it cannot express a table, a form control or a
//! block whose width depends on its parent. A browser view needs the
//! element structure to lay the page out, so this module keeps a small
//! tree of elements and text.
//!
//! Everything is fixed-capacity and zero-allocation. Nodes, attributes,
//! decoded text and the stylesheet text live in arrays owned by [`Dom`].
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

/// The most bytes of stylesheet text one document may carry, from its own
/// `<style>` tags and from the files it links.
pub const MAX_CSS_BYTES: usize = 16 * 1024;

/// The most `<link rel="stylesheet">` addresses one document may name.
///
/// WHY a handful rather than one: a page that chains its styles is ordinary,
/// and the file that carries its colours and its type is not always the one
/// it asks for first. WHY not many: each is fetched before the page is drawn,
/// so a long chain is a slow blank screen instead.
pub const MAX_STYLESHEET_LINKS: usize = 4;

/// The most `<link rel="icon">` addresses one document may name.
///
/// WHY more than one: a page lists its picture in the formats it likes best,
/// and the first is often one this browser has no decoder for. Four reaches
/// a drawable file past the vector variant and the sizes the page names for
/// other screens, without turning the icon into four more fetches.
pub const MAX_ICON_LINKS: usize = 4;

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

/// Which of the tree's own budgets was reached while parsing.
///
/// WHY named rather than folded into `truncated`: the flag says the document
/// is not all here, and that is the honest thing to tell a reader. It is not
/// the thing to tell anyone trying to fix it, because the budgets fail for
/// different reasons -- a page with ten thousand elements and a page with one
/// enormous stylesheet both say `true`.
pub const OVERFLOW_NODES: u32 = 1 << 0;
pub const OVERFLOW_TEXT: u32 = 1 << 1;
pub const OVERFLOW_ATTRS: u32 = 1 << 2;
pub const OVERFLOW_CSS: u32 = 1 << 3;
pub const OVERFLOW_DEPTH: u32 = 1 << 4;

/// What a document says about itself.
///
/// WHY this is kept beside the tree instead of in it: none of it is body
/// content, so the reader never draws it -- but the tab, the history list, the
/// download sheet and the start page all need a name for the page, and until
/// now the only name any of them had was the URL it was fetched from.
///
/// Every field is a range in the document's text pool, already entity-decoded
/// and reduced to the glyphs the face can draw. An empty range means the
/// document did not say, which is reported as such rather than guessed.
#[derive(Clone, Copy, Debug)]
pub struct DocMeta {
    /// The text of the first `<title>`, or of `og:title` when it has none.
    pub title: StrRef,
    /// `meta[name="description"]` or `og:description`.
    pub description: StrRef,
    /// The hrefs of the `<link rel="icon">` tags, in the order the document
    /// gave them. `icon_count` says how many of the four it actually named.
    pub icons: [StrRef; MAX_ICON_LINKS],
    pub icon_count: u8,
    /// The hrefs of the `<link rel="stylesheet">` tags, in the order the
    /// document gave them. `stylesheet_count` says how many of the four it
    /// actually named.
    pub stylesheets: [StrRef; MAX_STYLESHEET_LINKS],
    pub stylesheet_count: u8,
}

impl DocMeta {
    pub const EMPTY: Self = Self {
        title: StrRef::EMPTY,
        description: StrRef::EMPTY,
        icons: [StrRef::EMPTY; MAX_ICON_LINKS],
        icon_count: 0,
        stylesheets: [StrRef::EMPTY; MAX_STYLESHEET_LINKS],
        stylesheet_count: 0,
    };
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
    meta: DocMeta,
    root: NodeId,
    truncated: bool,
    overflow: u32,
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
            meta: DocMeta::EMPTY,
            root: NO_NODE,
            truncated: false,
            overflow: 0,
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

            // Raw-text elements are not markup. A script body is captured
            // into the DOM so the JS engine can execute it; the first
            // stylesheet is kept because layout needs it; a textarea's body is text.
            if tag.name_eq(b"script") {
                let start = i;
                let close_idx = find_close(html, i, b"script").unwrap_or(n);
                let end = find_close_start(html, start, b"script").unwrap_or(close_idx);
                i = close_idx;
                let node = self.open_element(&tag, &mut stack, &mut depth);
                if node != NO_NODE && end > start {
                    self.push_raw_text(node, &html[start..end]);
                }
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
                let start = i;
                i = find_close(html, i, b"title").unwrap_or(n);
                let end = find_close_start(html, start, b"title").unwrap_or(i);
                // WHY the title is kept on the document instead of in the tree:
                // it is not body content, so the reader must never draw it --
                // but the tab, the history entry and the download sheet are all
                // asking what this page is called, and nothing but this ever
                // knew. The first title wins, which is what the markup's own
                // order asks for.
                if end > start && self.meta.title.is_empty() {
                    let mut line = [0u8; 256];
                    let len = one_line(&html[start..end], &mut line);
                    if len > 0 {
                        let title = self.decode_display(&line[..len]);
                        self.meta.title = title;
                    }
                }
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
                self.note(OVERFLOW_DEPTH);
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
        self.meta = DocMeta::EMPTY;
        self.root = NO_NODE;
        self.truncated = false;
        self.overflow = 0;
    }

    /// Records which budget was reached. Every call is also a truncation, so
    /// the readers that only ask `truncated` keep working.
    fn note(&mut self, budget: u32) {
        self.overflow |= budget;
        self.truncated = true;
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

    /// What the document said about itself.
    pub fn meta(&self) -> &DocMeta {
        &self.meta
    }

    /// The stylesheets the document linked, in the order it linked them.
    pub fn stylesheets(&self) -> &[StrRef] {
        let count = (self.meta.stylesheet_count as usize).min(MAX_STYLESHEET_LINKS);
        &self.meta.stylesheets[..count]
    }

    /// The pictures the document offered as its own, in the order it offered
    /// them.
    pub fn icons(&self) -> &[StrRef] {
        let count = (self.meta.icon_count as usize).min(MAX_ICON_LINKS);
        &self.meta.icons[..count]
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

    /// Sets or updates an attribute on an element node.
    pub fn set_attr(&mut self, id: NodeId, name: &[u8], value: &[u8]) -> bool {
        let idx = id as usize;
        if idx >= self.node_count { return false; }
        let start = self.nodes[idx].attr_start as usize;
        let count = self.nodes[idx].attr_count as usize;
        let end = (start + count).min(self.attr_count);
        for i in start..end {
            if self.attrs[i].name.get(self).eq_ignore_ascii_case(name) {
                if self.text_len + value.len() <= MAX_TEXT_BYTES {
                    let v_start = self.text_len as u32;
                    let v_len = value.len() as u16;
                    self.text[self.text_len..self.text_len + value.len()].copy_from_slice(value);
                    self.text_len += value.len();
                    self.attrs[i].value = StrRef { start: v_start, len: v_len };
                    return true;
                }
                return false;
            }
        }
        if self.attr_count < MAX_ATTRS && self.text_len + name.len() + value.len() <= MAX_TEXT_BYTES {
            let n_start = self.text_len as u32;
            let n_len = name.len() as u16;
            self.text[self.text_len..self.text_len + name.len()].copy_from_slice(name);
            self.text_len += name.len();

            let v_start = self.text_len as u32;
            let v_len = value.len() as u16;
            self.text[self.text_len..self.text_len + value.len()].copy_from_slice(value);
            self.text_len += value.len();

            let attr_idx = self.attr_count;
            self.attrs[attr_idx] = Attr {
                name: StrRef { start: n_start, len: n_len },
                value: StrRef { start: v_start, len: v_len },
            };
            self.attr_count += 1;
            if self.nodes[idx].attr_count == 0 {
                self.nodes[idx].attr_start = attr_idx as u16;
            }
            self.nodes[idx].attr_count += 1;
            true
        } else {
            false
        }
    }

    /// Sets or replaces the text content of a node.
    pub fn set_text(&mut self, id: NodeId, new_text: &[u8]) -> bool {
        let idx = id as usize;
        if idx >= self.node_count { return false; }
        if self.text_len + new_text.len() <= MAX_TEXT_BYTES {
            let start = self.text_len as u32;
            let len = new_text.len() as u16;
            self.text[self.text_len..self.text_len + new_text.len()].copy_from_slice(new_text);
            self.text_len += new_text.len();
            if self.nodes[idx].is_text() {
                self.nodes[idx].text = StrRef { start, len };
                true
            } else if let Some(child_id) = self.first_child(id) {
                let child_idx = child_id as usize;
                if self.nodes[child_idx].is_text() {
                    self.nodes[child_idx].text = StrRef { start, len };
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
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

    /// The stylesheet text this document is styled by: what it wrote between
    /// its own `<style>` tags, then what its links brought.
    pub fn css(&self) -> &[u8] {
        &self.css[..self.css_len]
    }

    /// The body text of a script element, or empty if none.
    pub fn script_body<'a>(&'a self, node: NodeId) -> &'a [u8] {
        let mut child = self.first_child(node);
        while let Some(c) = child {
            if self.node(c).is_text() {
                return self.text_of(c);
            }
            child = self.next_sibling(c);
        }
        b""
    }

    /// Whether any capacity was reached while parsing.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Which capacities were reached, as `OVERFLOW_*` for this module.
    pub fn overflow(&self) -> u32 {
        self.overflow
    }

    fn capture_css(&mut self, css: &[u8]) {
        let n = css.len().min(MAX_CSS_BYTES);
        self.css[..n].copy_from_slice(&css[..n]);
        self.css_len = n;
        if css.len() > n {
            self.note(OVERFLOW_CSS);
        }
    }

    /// Adds a linked stylesheet's rules after the ones the page carried.
    ///
    /// WHY appended and not swapped in: what a page writes between its own
    /// `<style>` tags and what it links from a file are both part of how it
    /// wants to look, and the cascade reads the two as one text. Replacing
    /// would drop whichever arrived second.
    ///
    /// The arena is bounded, so rules that do not fit are not applied and
    /// the overflow is recorded rather than hidden. Returns how many of the
    /// given bytes were kept, which is fewer than the input once the arena
    /// is full.
    pub fn append_css(&mut self, css: &[u8]) -> usize {
        if css.is_empty() {
            return 0;
        }
        // The separator costs one byte and buys the parser a clean edge
        // between two stylesheets that are being read as one.
        if self.css_len > 0 && self.css_len < MAX_CSS_BYTES {
            self.css[self.css_len] = b'\n';
            self.css_len += 1;
        }
        let free = MAX_CSS_BYTES - self.css_len;
        let n = css.len().min(free);
        self.css[self.css_len..self.css_len + n].copy_from_slice(&css[..n]);
        self.css_len += n;
        if css.len() > n {
            self.note(OVERFLOW_CSS);
        }
        n
    }

    fn push_node(&mut self, kind: NodeKind, tag: StrRef, text: StrRef) -> NodeId {
        if self.node_count >= MAX_NODES {
            self.note(OVERFLOW_NODES);
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
            self.note(OVERFLOW_TEXT);
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
        self.decode_entities_into(raw, true);
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

    fn push_raw_text(&mut self, parent: NodeId, raw: &[u8]) {
        if parent == NO_NODE || raw.is_empty() {
            return;
        }
        let start = self.text_len;
        let room = MAX_TEXT_BYTES.saturating_sub(self.text_len);
        let n = raw.len().min(room);
        if n < raw.len() {
            self.note(OVERFLOW_TEXT);
        }
        if n == 0 {
            return;
        }
        self.text[start..start + n].copy_from_slice(&raw[..n]);
        self.text_len += n;
        let text = StrRef {
            start: start as u32,
            len: n as u16,
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
                self.note(OVERFLOW_ATTRS);
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
        self.read_document_meta(id);
        let parent = stack[*depth - 1];
        self.append_child(parent, id);
        id
    }

    /// Records what a `<meta>` or `<link>` says about the document.
    ///
    /// WHY the attribute values are copied into local buffers first: `attr`
    /// lends a slice of the text pool, and storing what it says needs to write
    /// into that same pool.
    fn read_document_meta(&mut self, node: NodeId) {
        let tag = self.tag(node);
        let is_meta = tag.eq_ignore_ascii_case(b"meta");
        let is_link = !is_meta && tag.eq_ignore_ascii_case(b"link");
        if !is_meta && !is_link {
            return;
        }
        let mut key = [0u8; 32];
        let mut rel = [0u8; 64];
        let mut content = [0u8; 256];
        let mut href = [0u8; 256];
        let mut key_len = attr_into(self, node, b"name", &mut key);
        if key_len == 0 {
            // Open Graph and the other social dialects spell their key
            // `property` instead of `name`; either one means the same thing here.
            key_len = attr_into(self, node, b"property", &mut key);
        }
        let rel_len = attr_into(self, node, b"rel", &mut rel);
        let content_len = attr_into(self, node, b"content", &mut content);
        let href_len = attr_into(self, node, b"href", &mut href);

        if is_meta {
            if content_len == 0 {
                return;
            }
            let key = &key[..key_len];
            let content = &content[..content_len];
            if key.eq_ignore_ascii_case(b"description") || key.eq_ignore_ascii_case(b"og:description") {
                let description = self.decode_display(content);
                self.meta.description = description;
            } else if key.eq_ignore_ascii_case(b"og:title") && self.meta.title.is_empty() {
                let title = self.decode_display(content);
                self.meta.title = title;
            }
        } else if href_len > 0 {
            let rel = &rel[..rel_len];
            let href = &href[..href_len];
            if rel_contains(rel, b"icon") || rel_contains(rel, b"apple-touch-icon") {
                self.link_icon(href);
            } else if rel_contains(rel, b"stylesheet") {
                self.link_stylesheet(href);
            }
        }
    }

    /// Remembers one `<link rel="icon">` address, in document order.
    ///
    /// WHY the list rather than the first: a page names the picture it is
    /// known by several times over, in the formats it likes best, and the
    /// first is very often one this browser cannot draw. The reader tries
    /// them in the order given and takes the one that arrives as a picture.
    fn link_icon(&mut self, href: &[u8]) {
        let count = (self.meta.icon_count as usize).min(MAX_ICON_LINKS);
        if count == MAX_ICON_LINKS {
            return;
        }
        let icon = self.intern(href);
        self.meta.icons[count] = icon;
        self.meta.icon_count = count as u8 + 1;
    }

    /// Remembers one `<link rel="stylesheet">` address, in document order.
    ///
    /// WHY the list stops at [`MAX_STYLESHEET_LINKS`] rather than overwriting
    /// the last slot: the reader fetches these before it draws anything, so
    /// what it holds has to be what the page asked for first, not whatever it
    /// happened to reach last.
    fn link_stylesheet(&mut self, href: &[u8]) {
        let count = (self.meta.stylesheet_count as usize).min(MAX_STYLESHEET_LINKS);
        if count == MAX_STYLESHEET_LINKS {
            return;
        }
        let sheet = self.intern(href);
        self.meta.stylesheets[count] = sheet;
        self.meta.stylesheet_count = count as u8 + 1;
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
        self.decode_entities_into(raw, false);
        StrRef {
            start: start as u32,
            len: (self.text_len - start) as u16,
        }
    }

    /// Appends `raw` to the end of the pool as text a person will read.
    fn decode_display(&mut self, raw: &[u8]) -> StrRef {
        let start = self.text_len;
        self.decode_entities_into(raw, true);
        StrRef {
            start: start as u32,
            len: (self.text_len - start).min(u16::MAX as usize) as u16,
        }
    }

    /// Decodes the character references a page uses and appends them.
    ///
    /// WHY `to_display` is a flag and not two functions: the two callers differ
    /// only in what the bytes are for. An attribute value is a string the reader
    /// never sees and the fetcher must send unchanged -- transliterating an `href`
    /// would ask for `/cafe%CC%81`'s neighbour `/cafe` and quietly break the link.
    /// Text content is drawn, so it is reduced to what the drawing face has.
    fn decode_entities_into(&mut self, raw: &[u8], to_display: bool) {
        let mut i = 0usize;
        while i < raw.len() {
            if raw[i] == b'&' {
                if let Some((decoded, consumed)) = decode_entity(&raw[i..]) {
                    if !self.push_char(decoded, to_display) {
                        return;
                    }
                    i += consumed;
                    continue;
                }
            }
            // WHY the whole sequence is stepped over, not one byte: a UTF-8
            // continuation byte on its own is not a character, and appending it
            // is what drew a hollow box per fragment of an accented letter.
            if to_display && raw[i] >= 0x80 {
                let (cp, next) = crate::decode_utf8_at(raw, i);
                if next > i {
                    if cp >= 0x80 {
                        for &b in crate::transliterate(cp) {
                            if !self.push_byte(b) {
                                return;
                            }
                        }
                    }
                    i = next;
                    continue;
                }
            }
            if !self.push_byte(raw[i]) {
                return;
            }
            i += 1;
        }
    }

    /// Appends one byte to the text arena, reporting whether there was room.
    fn push_byte(&mut self, b: u8) -> bool {
        if self.text_len < MAX_TEXT_BYTES {
            self.text[self.text_len] = b;
            self.text_len += 1;
            true
        } else {
            self.note(OVERFLOW_TEXT);
            false
        }
    }

    /// Appends a character reference's value.
    ///
    /// Returns false once the arena is full, so the caller stops rather than
    /// silently dropping the rest of a node's text.
    fn push_char(&mut self, ch: char, to_display: bool) -> bool {
        if to_display && !ch.is_ascii() {
            for &b in crate::transliterate(ch as u32) {
                if !self.push_byte(b) {
                    return false;
                }
            }
            return true;
        }
        let mut buf = [0u8; 4];
        let bytes = ch.encode_utf8(&mut buf).as_bytes();
        for &b in bytes {
            if !self.push_byte(b) {
                return false;
            }
        }
        true
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

    #[allow(dead_code)]
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

/// Copies one attribute's value into a fixed buffer and returns how much of it
/// fitted. A missing or empty attribute reads as zero bytes.
fn attr_into(dom: &Dom, node: NodeId, name: &[u8], dst: &mut [u8]) -> usize {
    match dom.attr(node, name) {
        Some(value) if !value.is_empty() => {
            let n = value.len().min(dst.len());
            dst[..n].copy_from_slice(&value[..n]);
            n
        }
        _ => 0,
    }
}

/// Copies `raw` into `dst` as a single line: the whitespace a page's authors
/// put around the title is dropped and the runs inside it become one space,
/// because the tab and the history row have exactly one line to show it on.
fn one_line(raw: &[u8], dst: &mut [u8]) -> usize {
    let mut len = 0usize;
    let mut pending_space = false;
    for &byte in raw {
        if byte == b' ' || byte == b'\t' || byte == b'\n' || byte == b'\r' || byte == b'\x0c' {
            pending_space = len > 0;
            continue;
        }
        if pending_space {
            if len == dst.len() {
                break;
            }
            dst[len] = b' ';
            len += 1;
            pending_space = false;
        }
        if len == dst.len() {
            break;
        }
        dst[len] = byte;
        len += 1;
    }
    len
}

/// Whether a `rel` list names `keyword` as one of its space-separated tokens.
///
/// WHY tokens and not a substring: `rel="alternate stylesheet"` has to match,
/// and a plain contains() would also let `rel="next-stylesheet"` claim it.
fn rel_contains(rel: &[u8], keyword: &[u8]) -> bool {
    rel.split(|&byte| byte == b' ' || byte == b'\t' || byte == b'\n')
        .any(|token| token.eq_ignore_ascii_case(keyword))
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
            | b"content"
            | b"rel"
            | b"property"
            | b"charset"
            | b"checked"
            | b"selected"
            | b"disabled"
            | b"hidden"
            | b"size"
            | b"maxlength"
            | b"onclick"
            | b"onsubmit"
            | b"onchange"
            | b"oninput"
            | b"onload"
    ) || name.starts_with(b"on")
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
    fn drawn_text_is_reduced_to_glyphs_the_face_has() {
        // "café" is 0x63 0x61 0x66 0xC3 0xA9. The drawing face is the ASCII
        // bitmap cell, so the two bytes of the é would each paint a hollow box
        // and the word would read "caf[]". Transliterating the code point, not
        // the byte, is what keeps the word readable.
        let mut dom = Dom::new();
        dom.parse("<p>café &mdash; naïve</p>".as_bytes());
        let p = find(&dom, b"p").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, p, &mut text);
        assert_eq!(text, b"cafe -- naive");
    }

    #[test]
    fn an_href_keeps_the_bytes_the_server_wants() {
        // Transliterating a URL is a request for a different document, so the
        // attribute path decodes entities and nothing else.
        let mut dom = Dom::new();
        dom.parse("<a href=\"/café&amp;x\">link</a>".as_bytes());
        let a = find(&dom, b"a").unwrap();
        assert_eq!(dom.attr(a, b"href"), Some("/café&x".as_bytes()));
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
    fn the_first_stylesheet_is_kept_and_a_script_is_captured() {
        let mut dom = Dom::new();
        dom.parse(b"<style>p{color:#112233}</style><p>ok</p><script>var x='<p>no</p>';</script>");
        assert_eq!(dom.css(), b"p{color:#112233}");
        let p = find(&dom, b"p").unwrap();
        let mut text = std::vec::Vec::new();
        collect_text(&dom, p, &mut text);
        assert_eq!(text, b"ok");
        let script = find(&dom, b"script").expect("script element must be captured into DOM");
        assert_eq!(dom.script_body(script), b"var x='<p>no</p>';");
    }

    #[test]
    fn a_linked_stylesheet_lands_after_the_inline_one() {
        let mut dom = Dom::new();
        dom.parse(b"<style>p{color:#112233}</style>");
        assert_eq!(dom.append_css(b"h1{font-weight:bold}"), 20);
        assert_eq!(
            dom.css(),
            b"p{color:#112233}\nh1{font-weight:bold}",
            "the two are read as one cascade, with an edge between them"
        );
    }

    #[test]
    fn a_page_with_no_inline_style_starts_its_arena_at_the_link() {
        let mut dom = Dom::new();
        dom.parse(b"<p>plain</p>");
        assert_eq!(dom.append_css(b"p{color:red}"), 12);
        assert_eq!(dom.css(), b"p{color:red}", "no leading separator");
    }

    #[test]
    fn a_stylesheet_that_does_not_fit_is_said_not_to_have_fit() {
        let mut dom = Dom::new();
        dom.parse(b"<p>plain</p>");
        let too_big = std::vec![b'x'; MAX_CSS_BYTES + 8];
        assert_eq!(dom.append_css(&too_big), MAX_CSS_BYTES);
        assert_eq!(dom.css().len(), MAX_CSS_BYTES);
        assert_ne!(dom.overflow() & OVERFLOW_CSS, 0);
    }

    #[test]
    fn script_tag_retains_type_attribute() {
        let mut dom = Dom::new();
        dom.parse(b"<script type=\"application/ld+json\">{\"key\":\"value\"}</script>");
        let script = find(&dom, b"script").expect("script element must be captured into DOM");
        assert_eq!(dom.attr(script, b"type"), Some(&b"application/ld+json"[..]));
        assert_eq!(dom.script_body(script), b"{\"key\":\"value\"}");
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
        // The no-break space arrives as U+00A0 and leaves as the space the face
        // can draw; the two around it are the ones the markup already had.
        assert_eq!(text, b"<x> A B   &unknown;");
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

    #[test]
    fn the_title_is_read_off_the_document_and_never_drawn() {
        let mut dom = Dom::new();
        dom.parse(b"<head><title>Weather &amp; Tides</title></head><body><p>sea</p></body>");
        assert_eq!(dom.meta().title.get(&dom), b"Weather & Tides");
        // The body of the title is not content: nothing in the tree may show it.
        assert!(find(&dom, b"title").is_none());
        let mut text = std::vec::Vec::new();
        collect_text(&dom, dom.root(), &mut text);
        assert_eq!(text, b"sea");
    }

    #[test]
    fn a_title_the_authors_indented_still_fits_one_line() {
        let mut dom = Dom::new();
        dom.parse(b"<title>\n   InterOS\n   Core\n</title>");
        assert_eq!(dom.meta().title.get(&dom), b"InterOS Core");
    }

    #[test]
    fn the_first_title_wins() {
        let mut dom = Dom::new();
        dom.parse(b"<title>First</title><title>Second</title>");
        assert_eq!(dom.meta().title.get(&dom), b"First");
    }

    #[test]
    fn a_social_title_names_the_page_when_it_says_none() {
        let mut dom = Dom::new();
        dom.parse(b"<meta property=\"og:title\" content=\"Shared page\">");
        assert_eq!(dom.meta().title.get(&dom), b"Shared page");
        // A real title is the page's own word on itself, so it is not overwritten.
        dom.parse(b"<title>Named</title><meta property=\"og:title\" content=\"Shared\">");
        assert_eq!(dom.meta().title.get(&dom), b"Named");
    }

    #[test]
    fn the_description_is_read_from_either_spelling() {
        let mut dom = Dom::new();
        dom.parse(b"<meta name=\"description\" content=\"A short summary.\">");
        assert_eq!(dom.meta().description.get(&dom), b"A short summary.");
        dom.parse(b"<meta property=\"og:description\" content=\"Another summary.\">");
        assert_eq!(dom.meta().description.get(&dom), b"Another summary.");
    }

    #[test]
    fn the_icon_and_stylesheet_hrefs_are_read_as_the_page_gives_them() {
        let mut dom = Dom::new();
        dom.parse(b"<link rel=\"shortcut icon\" href=\"/f.ico\"><link REL=\"Alternate Stylesheet\" href=\"/a.css\">");
        assert_eq!(dom.icons()[0].get(&dom), b"/f.ico");
        assert_eq!(dom.stylesheets()[0].get(&dom), b"/a.css");
    }

    #[test]
    fn a_page_that_offers_its_picture_in_several_formats_keeps_them_all() {
        let mut dom = Dom::new();
        dom.parse(b"<link rel=\"icon\" type=\"image/svg+xml\" href=\"/f.svg\"><link rel=\"icon\" type=\"image/png\" sizes=\"32x32\" href=\"/f-32.png\"><link rel=\"apple-touch-icon\" href=\"/touch.png\">");
        let links: std::vec::Vec<_> = dom
            .icons()
            .iter()
            .map(|s| s.get(&dom))
            .collect();
        assert_eq!(
            links,
            std::vec![b"/f.svg".as_slice(), b"/f-32.png".as_slice(), b"/touch.png".as_slice()]
        );
    }

    #[test]
    fn every_stylesheet_the_page_links_is_kept_in_the_order_it_said() {
        let mut dom = Dom::new();
        dom.parse(b"<link rel=\"stylesheet\" href=\"/a.css\"><link rel=\"stylesheet\" href=\"/b.css\">");
        let links: std::vec::Vec<_> = dom
            .stylesheets()
            .iter()
            .map(|s| s.get(&dom))
            .collect();
        assert_eq!(links, std::vec![b"/a.css".as_slice(), b"/b.css".as_slice()]);
    }

    #[test]
    fn a_link_chain_longer_than_the_reader_holds_keeps_the_front_of_it() {
        let mut dom = Dom::new();
        dom.parse(
            b"<link rel=\"stylesheet\" href=\"/a.css\"><link rel=\"stylesheet\" href=\"/b.css\">\
              <link rel=\"stylesheet\" href=\"/c.css\"><link rel=\"stylesheet\" href=\"/d.css\">\
              <link rel=\"stylesheet\" href=\"/e.css\">",
        );
        assert_eq!(dom.stylesheets().len(), MAX_STYLESHEET_LINKS);
        assert_eq!(dom.stylesheets()[0].get(&dom), b"/a.css");
        assert_eq!(dom.stylesheets()[MAX_STYLESHEET_LINKS - 1].get(&dom), b"/d.css");
    }

    #[test]
    fn a_rel_that_only_rhymes_with_icon_is_not_the_icon() {
        let mut dom = Dom::new();
        dom.parse(b"<link rel=\"next-stylesheet\" href=\"/x.css\">");
        assert!(dom.icons().is_empty());
        assert!(dom.stylesheets().is_empty());
    }

    #[test]
    fn a_page_that_says_nothing_about_itself_is_reported_as_saying_nothing() {
        let mut dom = Dom::new();
        dom.parse(b"<body><p>plain</p></body>");
        assert!(dom.meta().title.is_empty());
        assert!(dom.meta().description.is_empty());
        assert!(dom.icons().is_empty());
        assert!(dom.stylesheets().is_empty());
    }

    #[test]
    fn the_document_metadata_does_not_survive_into_the_next_page() {
        let mut dom = Dom::new();
        dom.parse(b"<title>Old</title><meta name=\"description\" content=\"Old.\"><link rel=\"stylesheet\" href=\"/old.css\">");
        dom.parse(b"<p>New</p>");
        assert!(dom.meta().title.is_empty());
        assert!(dom.meta().description.is_empty());
        assert!(dom.stylesheets().is_empty());
    }
}
