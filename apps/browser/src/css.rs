// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! A bounded CSS subset for the native reader.
//!
//! WHY this is not a CSS engine: the reader lays a page out as a linear
//! column of text. What it can honestly apply is the small set of
//! properties that change those lines -- an ink colour, a background, a
//! font rung, a top margin, a left inset and a hairline border. Everything
//! else (flexbox, grid, floats, positioning, gradients, shadows,
//! transforms, animations) is not implemented and is ignored rather than
//! approximated.
//!
//! The parser is deliberately bounded: rule count, selector length,
//! declaration length and the scanned byte count all have caps, and a
//! rule that does not fit is dropped. No allocation is performed anywhere
//! in this module.
//!
//! Supported selectors are a tag, a `.class`, an `#id`, a compound of one
//! tag with at most one class and one id, and a whitespace descendant
//! chain of at most [`MAX_SELECTOR_PARTS`] compounds. A selector using
//! `>`, `+`, `~`, `:`, `::`, `[` or `*` is skipped.

/// The most rules a stylesheet may contribute.
///
/// WHY it is a count and not only a byte window: the rules a page writes for
/// properties this engine does not read are still walked to reach the ones it
/// can apply, so the budget is what that walk is allowed to cost.
pub const MAX_RULES: usize = 96;

/// The most compounds in one descendant selector.
pub const MAX_SELECTOR_PARTS: usize = 3;

/// The longest selector text parsed.
pub const MAX_SELECTOR_LEN: usize = 64;

/// The longest single declaration parsed (`property: value`).
pub const MAX_DECLARATION_LEN: usize = 96;

/// The most bytes of stylesheet text that are read.
///
/// WHY it matches the document's own budget: the rules are collected into one
/// text and parsed from it as a whole, so a smaller window here would stop
/// the reader short of the file it just made room for.
pub const MAX_CSS_BYTES: usize = crate::layout::dom::MAX_CSS_BYTES;

/// The most declarations read from one inline `style` attribute.
pub const MAX_INLINE_DECLARATIONS: usize = 16;

/// The specificity of an inline `style` attribute.
pub const SPEC_INLINE: u16 = 1000;

/// The most `--token` definitions a document may name.
///
/// WHY it is a count of its own: a theme file is one long list of names for the
/// colours a page paints with -- GitHub's leads with nearly two thousand of
/// them -- and a reader that kept every name would spend the room its rules
/// need on names no rule asked for.
///
/// WHY this many: the stylesheet text one read covers is [`MAX_CSS_BYTES`], and
/// a live page's token block measured on 2026-10-28 declares 959 distinct names
/// in a file of that size, of which 788 are paintable and land inside the
/// window. The table holds those 788 with room over, so it is the window that
/// cuts a theme short and not the count: a table smaller than the window drops
/// names the page asked for while bytes of the file went unread beside them.
pub const MAX_TOKENS: usize = 832;

/// The longest token name or token value that is kept.
///
/// WHY a value is dropped rather than cut: half of a colour is a colour, and
/// it is the wrong one.
pub const MAX_TOKEN_LEN: usize = 48;

/// One `--token` definition, as the range of text its name and value occupy.
///
/// WHY offsets and not slices: the definitions are read out of a stylesheet
/// while it is being cut down and used long afterwards, and the only place
/// they can live between those two moments is the document's own arena, which
/// cannot borrow from itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TokenSlot {
    pub name: (u16, u16),
    pub value: (u16, u16),
}

impl TokenSlot {
    pub const EMPTY: Self = Self { name: (0, 0), value: (0, 0) };
}

/// The `--token` definitions a document's styles named.
#[derive(Clone, Copy)]
pub struct TokenStore<'a> {
    pub slots: &'a [TokenSlot],
    pub text: &'a [u8],
}

impl<'a> TokenStore<'a> {
    pub const NONE: Self = Self { slots: &[], text: &[] };

    /// The value a name was given.
    ///
    /// WHY the first definition wins: a site that ships a light and a dark
    /// theme writes the same names into both files, and this reader cannot
    /// tell which theme the page is wearing. The file that comes first is the
    /// one the page asked for first, and taking the last would paint a dark
    /// page's colours onto the light surface the reader draws on.
    pub fn value(&self, name: &[u8]) -> Option<&'a [u8]> {
        for slot in self.slots {
            let (at, len) = (slot.name.0 as usize, slot.name.1 as usize);
            let (v_at, v_len) = (slot.value.0 as usize, slot.value.1 as usize);
            if len == 0 || v_at + v_len > self.text.len() || at + len > self.text.len() {
                continue;
            }
            if &self.text[at..at + len] == name {
                return Some(&self.text[v_at..v_at + v_len]);
            }
        }
        None
    }
}

/// Which part of a selector a name in the document is.
///
/// WHY the kind is asked for: `nav` is a tag on one page, a class on the next
/// and an id in the markup beside both, and a document that holds one of those
/// does not hold the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameKind {
    Tag,
    Class,
    Id,
}

/// How many names a document's bits answer for.
///
/// WHY this many: a live page counted on 2026-10-28 -- github.com's front door,
/// as far as one read of it reaches -- carries 25 distinct tags, 116 classes and
/// 19 ids. At that population the bits are barely filled, and a page that ships
/// its classes by the thousand still marks a name the document holds.
pub const NAME_BITS: usize = 8192;

/// How many words [`NAME_BITS`] bits occupy.
pub const NAME_WORDS: usize = NAME_BITS / 64;

/// Which bit a name is marked in.
///
/// WHY the case is folded: the reader matches tags, classes and ids without
/// regard to it, so `<div class="Hero">` and `.HERO` are the same name here as
/// they are there.
fn name_bit(kind: NameKind, name: &[u8]) -> (usize, u64) {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash = (hash ^ (kind as u64)).wrapping_mul(0x0000_0100_0000_01b3);
    let mut at = 0usize;
    while at < name.len() {
        hash = (hash ^ (name[at].to_ascii_lowercase() as u64)).wrapping_mul(0x0000_0100_0000_01b3);
        at += 1;
    }
    let index = (hash as usize) & (NAME_BITS - 1);
    (index / 64, 1u64 << (index % 64))
}

/// Marks a name the document carries.
pub fn mark_name(bits: &mut [u64], kind: NameKind, name: &[u8]) {
    if name.is_empty() {
        return;
    }
    let (word, bit) = name_bit(kind, name);
    if word < bits.len() {
        bits[word] |= bit;
    }
}

/// The names a document's elements carry, as one bit per name.
///
/// WHY bits and not the names: the only question ever asked of these is
/// whether the document holds a given one. An answer that is wrong in the
/// keeping direction costs a rule the reader would have ignored for another
/// reason; wrong in the dropping direction it costs a colour the page asked
/// for. A table of the names would have to be bounded, and a bound drops what
/// overflows it -- so the cheap one-way answer gets the room instead.
#[derive(Clone, Copy)]
pub struct NameSet<'a> {
    pub bits: &'a [u64],
    /// Whether a document was read into these bits at all.
    pub known: bool,
}

impl<'a> NameSet<'a> {
    pub const NONE: Self = Self { bits: &[], known: false };

    /// Whether the document carries this name.
    ///
    /// WHY an unknown document holds anything: a caller that never collected
    /// the names is not pruning, it is only asking whether a selector is
    /// readable. Refusing a name it never looked at would drop rules on a guess.
    pub fn holds(&self, kind: NameKind, name: &[u8]) -> bool {
        if !self.known || name.is_empty() {
            return true;
        }
        let (word, bit) = name_bit(kind, name);
        word < self.bits.len() && (self.bits[word] & bit) != 0
    }
}

/// Whether a declaration names a custom property, and the name it gives it.
fn custom_property(property: &[u8]) -> Option<&[u8]> {
    let property = trim(property);
    if property.len() > 2 && property.starts_with(b"--") {
        Some(property)
    } else {
        None
    }
}

/// Whether a run of digits is a length this engine's properties take.
fn length_value(value: &[u8]) -> bool {
    let mut digits = 0usize;
    let mut at = 0usize;
    while at < value.len() && (value[at].is_ascii_digit() || value[at] == b'.') {
        if value[at].is_ascii_digit() {
            digits += 1;
        }
        at += 1;
    }
    digits > 0 && matches!(&value[at..], b"" | b"px" | b"em" | b"rem" | b"%")
}

/// Whether a token's value is one this reader could paint if it found it.
///
/// WHY the test is at the door and not at the rule: a theme file names its
/// shadows, its font stacks and its gradient curve in the same list as its
/// colours, and a table filled with values no property can use is a table with
/// no room left for the ones a rule does ask for. A chain is kept because the
/// answer at the end of it may well be a colour.
fn paintable(value: &[u8]) -> bool {
    if value.len() > MAX_TOKEN_LEN {
        return false;
    }
    value.starts_with(b"var(") || parse_color(value).is_some() || length_value(value)
}

/// Replaces a `var(--name)` or `var(--name, fallback)` with what it stands for.
///
/// WHY only a whole value: a token inside a list (`margin: 0 var(--x)`) would
/// need somewhere to put the bytes it becomes, and every value this reader
/// applies is a single token -- one colour, one size, one font rung. A `var()`
/// that is part of a larger value is handed back as it was, and the
/// declaration is then refused for what it is: a value this reader cannot
/// resolve.
///
/// WHY a chain is walked: a theme names its colours after other names
/// (`--fgColor-muted: var(--color-fg-muted)`), so the answer to one question
/// can be another question. Four hops is where the walk stops, because a page
/// that defines a token in terms of itself has no answer.
pub fn resolve_custom<'a>(tokens: TokenStore<'a>, value: &'a [u8]) -> &'a [u8] {
    let mut value = trim(value);
    let mut hops = 0usize;
    while value.starts_with(b"var(") && hops < 4 {
        hops += 1;
        let inner = &value[4..];
        let Some(close) = inner.iter().rposition(|&b| b == b')') else {
            break;
        };
        let inner = &inner[..close];
        let (name, fallback) = match inner.iter().position(|&b| b == b',') {
            Some(comma) => (trim(&inner[..comma]), trim(&inner[comma + 1..])),
            None => (inner, b"" as &[u8]),
        };
        let named = custom_property(name).and_then(|name| tokens.value(name));
        value = trim(named.unwrap_or(fallback));
        if value.is_empty() {
            break;
        }
    }
    value
}

/// The reader's computed style for one block.
///
/// `font_scale` is zero when no rule or inline declaration set a size;
/// the layout then keeps its own rung. A value of one or two names the
/// platform type ladder rung the declaration asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ComputedStyle {
    pub color: u32,
    pub has_color: bool,
    pub background: u32,
    pub has_background: bool,
    pub font_scale: u8,
    pub margin_top: u8,
    pub padding_left: u8,
    pub border_color: u32,
    pub has_border: bool,
}

impl ComputedStyle {
    pub const DEFAULT: Self = Self {
        color: 0,
        has_color: false,
        background: 0,
        has_background: false,
        font_scale: 0,
        margin_top: 0,
        padding_left: 0,
        border_color: 0,
        has_border: false,
    };

    /// Whether the style changes anything the reader can paint.
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

/// One compound of a selector: an optional tag, class and id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Compound<'a> {
    pub tag: &'a [u8],
    pub class: &'a [u8],
    pub id: &'a [u8],
}

impl<'a> Compound<'a> {
    pub const EMPTY: Self = Self {
        tag: b"",
        class: b"",
        id: b"",
    };
}

/// A parsed selector and its specificity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selector<'a> {
    pub parts: [Compound<'a>; MAX_SELECTOR_PARTS],
    pub count: usize,
    pub specificity: u16,
}

impl<'a> Selector<'a> {
    pub const EMPTY: Self = Self {
        parts: [Compound::EMPTY; MAX_SELECTOR_PARTS],
        count: 0,
        specificity: 0,
    };
}

/// One rule: the selector that matches and the style it contributes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rule<'a> {
    pub selector: Selector<'a>,
    pub style: ComputedStyle,
}

impl<'a> Rule<'a> {
    pub const EMPTY: Self = Self {
        selector: Selector::EMPTY,
        style: ComputedStyle::DEFAULT,
    };
}

/// A bounded list of rules.
pub struct Stylesheet<'a> {
    pub rules: [Rule<'a>; MAX_RULES],
    pub len: usize,
    /// The `--token` definitions the document's styles named.
    ///
    /// WHY they ride with the sheet rather than with the text: a rule's colour
    /// is read when the sheet is parsed, and the name it points at lives in a
    /// different file than the rule that asks for it.
    pub tokens: TokenStore<'a>,
}

impl<'a> Stylesheet<'a> {
    pub const fn empty() -> Self {
        Self {
            rules: [Rule::EMPTY; MAX_RULES],
            len: 0,
            tokens: TokenStore::NONE,
        }
    }
}

/// Which properties a rule set, so a later rule can be outranked
/// property by property rather than as a whole.
#[derive(Clone, Copy)]
struct Specs {
    color: u16,
    background: u16,
    font: u16,
    margin: u16,
    padding: u16,
    border: u16,
}

impl Specs {
    const NONE: Self = Self {
        color: 0,
        background: 0,
        font: 0,
        margin: 0,
        padding: 0,
        border: 0,
    };
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' || bytes[i] == b'\r') {
        i += 1;
    }
    i
}

fn trim(mut value: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = value.split_first() {
        if first == b' ' || first == b'\t' || first == b'\n' || first == b'\r' {
            value = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = value.split_last() {
        if last == b' ' || last == b'\t' || last == b'\n' || last == b'\r' {
            value = rest;
        } else {
            break;
        }
    }
    value
}

fn eq_ignore_ascii(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Parses a CSS colour into `0xAARRGGBB`.
///
/// The reader's palette is monochrome except for the ink a page asks
/// for, so a small set of names and the hex forms are enough. Anything
/// else is not guessed at.
pub fn parse_color(value: &[u8]) -> Option<u32> {
    if value.is_empty() || value[0] != b'#' {
        return match value {
            b"black" => Some(0xFF000000),
            b"white" => Some(0xFFFFFFFF),
            b"red" => Some(0xFFD32F2F),
            b"green" => Some(0xFF2E7D32),
            b"blue" => Some(0xFF1565C0),
            b"yellow" => Some(0xFFF9A825),
            b"orange" => Some(0xFFEF6C00),
            b"purple" => Some(0xFF6A1B9A),
            b"teal" => Some(0xFF00695C),
            b"gray" | b"grey" => Some(0xFF757575),
            b"silver" => Some(0xFFBDBDBD),
            b"maroon" => Some(0xFF8E0000),
            b"navy" => Some(0xFF0D2C6B),
            b"transparent" => Some(0x00000000),
            _ => None,
        };
    }
    let hex = &value[1..];
    let digit = |b: u8| -> Option<u32> {
        match b {
            b'0'..=b'9' => Some((b - b'0') as u32),
            b'a'..=b'f' => Some((b - b'a' + 10) as u32),
            b'A'..=b'F' => Some((b - b'A' + 10) as u32),
            _ => None,
        }
    };
    match hex.len() {
        3 => {
            let r = digit(hex[0])?;
            let g = digit(hex[1])?;
            let b = digit(hex[2])?;
            Some(0xFF000000 | ((r * 17) << 16) | ((g * 17) << 8) | (b * 17))
        }
        6 => {
            let mut rgb = 0u32;
            for &b in hex {
                rgb = (rgb << 4) | digit(b)?;
            }
            Some(0xFF000000 | rgb)
        }
        _ => None,
    }
}

/// Returns the token a CSS box shorthand uses for one side.
///
/// A one-value shorthand applies to every side, so any index returns that
/// value; with more values the second is the horizontal inset, which is
/// the only horizontal side this layout carries.
fn shorthand_token(value: &[u8], index: usize) -> Option<&[u8]> {
    let mut tokens = value
        .split(|&b| b == b' ' || b == b'\t' || b == b'\n' || b == b'\r')
        .filter(|token| !token.is_empty());
    let first = tokens.next()?;
    if index == 0 {
        return Some(first);
    }
    let mut chosen = first;
    for (seen, token) in tokens.enumerate() {
        chosen = token;
        if seen + 1 >= index {
            break;
        }
    }
    Some(chosen)
}

/// Parses a leading pixel length, clamped to `max`.
fn parse_length(value: &[u8], max: u8) -> Option<u8> {
    let value = trim(value);
    let mut i = 0usize;
    let mut number = 0u32;
    let mut any = false;
    while i < value.len() && value[i].is_ascii_digit() && i < 4 {
        number = number * 10 + (value[i] - b'0') as u32;
        any = true;
        i += 1;
    }
    if !any {
        return None;
    }
    // A percentage or a unit we do not implement is not guessed at. The
    // `px` suffix and a bare number are the two forms accepted.
    if i < value.len() && !eq_ignore_ascii(&value[i..], b"px") {
        // A unit the layout does not convert is not guessed at.
        return None;
    }
    Some(number.min(max as u32) as u8)
}

/// Maps a font size onto the platform's two practical body rungs.
///
/// The bitmap face has one rung that is legible as body copy (scale 1)
/// and one that reads as a title (scale 2); the document already uses
/// scale 2 for headings. A declared size below eighteen pixels keeps
/// body text at the body rung, and eighteen or above moves it to the
/// title rung. Sizes outside 8..=40 are clamped.
pub fn font_scale_for(value: &[u8]) -> Option<u8> {
    let value = trim(value);
    if eq_ignore_ascii(value, b"small") || eq_ignore_ascii(value, b"medium") {
        return Some(1);
    }
    if eq_ignore_ascii(value, b"large") || eq_ignore_ascii(value, b"x-large") {
        return Some(2);
    }
    let mut i = 0usize;
    let mut number = 0u32;
    let mut any = false;
    while i < value.len() && value[i].is_ascii_digit() && i < 4 {
        number = number * 10 + (value[i] - b'0') as u32;
        any = true;
        i += 1;
    }
    if !any {
        return None;
    }
    if i < value.len() {
        let unit = trim(&value[i..]);
        if !unit.is_empty() && !eq_ignore_ascii(unit, b"px") {
            return None;
        }
    }
    let number = number.clamp(8, 40);
    Some(if number >= 18 { 2 } else { 1 })
}

/// Applies one `property: value` declaration to `style`.
///
/// A property this subset does not implement is ignored, exactly as a
/// browser ignores a property its engine does not know. A value written as
/// `var(--name)` is read through `tokens` first, because that is where the
/// page wrote the colour the name stands for.
pub fn apply_declaration<'a>(
    style: &mut ComputedStyle,
    property: &[u8],
    value: &'a [u8],
    tokens: TokenStore<'a>,
) {
    let property = trim(property);
    let value = resolve_custom(tokens, value);
    if property.is_empty() || value.is_empty() {
        return;
    }
    if custom_property(property).is_some() {
        // A custom property sets no pixel: the reader keeps its value when it
        // stows the name, and applying it here would change nothing.
        return;
    }
    if eq_ignore_ascii(property, b"color") {
        if let Some(c) = parse_color(value).filter(|c| *c != 0) {
            style.color = c;
            style.has_color = true;
        }
    } else if eq_ignore_ascii(property, b"background-color")
        || (eq_ignore_ascii(property, b"background") && parse_color(value).is_some())
    {
        if let Some(c) = parse_color(value).filter(|c| *c != 0) {
            style.background = c;
            style.has_background = true;
        }
    } else if eq_ignore_ascii(property, b"font-size") {
        if let Some(scale) = font_scale_for(value) {
            style.font_scale = scale;
        }
    } else if eq_ignore_ascii(property, b"margin") || eq_ignore_ascii(property, b"margin-top") {
        // Only the top margin changes a linear reader's block spacing.
        // The first number of the shorthand is the top margin.
        let value = if eq_ignore_ascii(property, b"margin") {
            shorthand_token(value, 0).unwrap_or(value)
        } else {
            value
        };
        if let Some(n) = parse_length(value, 32) {
            style.margin_top = n;
        }
    } else if eq_ignore_ascii(property, b"padding") || eq_ignore_ascii(property, b"padding-left") {
        // Only the left inset is applied; a vertical padding that the
        // line layout does not model is not invented. In the shorthand,
        // `padding: 10px 4px` means vertical 10 and horizontal 4, so the
        // left inset is the second value when one is present.
        let value = if eq_ignore_ascii(property, b"padding") {
            shorthand_token(value, 1).unwrap_or(value)
        } else {
            value
        };
        if let Some(n) = parse_length(value, 32) {
            style.padding_left = n;
        }
    } else if eq_ignore_ascii(property, b"border") {
        apply_border(style, value);
    }
    // Every other property is intentionally ignored.
}

fn apply_border(style: &mut ComputedStyle, value: &[u8]) {
    if eq_ignore_ascii(value, b"none") || value.starts_with(b"0") {
        style.has_border = false;
        return;
    }
    // A width above three pixels is not drawn: the reader's hairline is
    // one logical pixel and a declared thick border is clamped, not
    // faked with a shadow.
    let mut color = None;
    for token in value.split(|&b| b == b' ' || b == b'\t' || b == b'\n' || b == b'\r') {
        let token = trim(token);
        if token.is_empty() || token[0].is_ascii_digit() || eq_ignore_ascii(token, b"solid")
            || eq_ignore_ascii(token, b"thin") || eq_ignore_ascii(token, b"medium")
            || eq_ignore_ascii(token, b"thick")
        {
            continue;
        }
        if let Some(c) = parse_color(token) {
            if c != 0x00000000 {
                color = Some(c);
            }
        }
    }
    style.border_color = color.unwrap_or_else(|| {
        if style.has_color {
            style.color
        } else {
            0xFF9E9E9E
        }
    });
    style.has_border = true;
}

/// The surface a document line is painted on when neither that line nor the
/// page names one.
///
/// WHY it comes from the theme and is not a number kept here: the renderer
/// fills every band that carries no background with exactly this colour, so a
/// shade measured against it is asked the question the pixels will answer.
pub const PAGE_SURFACE: u32 = crate::theme::THEME.surface;

/// The ink the reader writes with when no rule named one.
pub const PAGE_INK: u32 = crate::theme::THEME.on_surface;

/// How much light a colour carries, on the reader's measure.
///
/// WHY the channels are squared rather than raised to the sRGB power: the real
/// curve is a power of 2.4 and this engine keeps no floating point. A square
/// orders the lights the same way and is close enough at the ends of the scale,
/// which is where the only question this answers -- is this ink near the colour
/// it sits on -- is asked.
const fn light_of(color: u32) -> u32 {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    (54 * r * r + 183 * g * g + 18 * b * b) >> 8
}

/// The step added to both lights before they are compared, so that two fields
/// a little apart are not read as one.
const LIGHT_STEP: u32 = light_of(0x00FF_FFFF) / 20;

/// Whether `color` can be read on `surface`.
///
/// WHY the bar sits where it does: it asks the lighter field to be twice the
/// darker one above [`LIGHT_STEP`] -- near a 2:1 contrast ratio, which is where
/// a colour stops being a shade of what lies behind it. It is not a WCAG check
/// and is not offered as one: the decision here is only whether to paint a
/// colour at all, and a colour this surface cannot show is one the page has to
/// do without.
pub fn ink_shows(color: u32, surface: u32) -> bool {
    let ink = light_of(color);
    let behind = light_of(surface);
    let (higher, lower) = if ink >= behind { (ink, behind) } else { (behind, ink) };
    higher + LIGHT_STEP >= 2 * (lower + LIGHT_STEP)
}

/// The colour a style's own text is written on.
pub fn band_surface(style: &ComputedStyle) -> u32 {
    if style.has_background {
        style.background
    } else {
        PAGE_SURFACE
    }
}

/// Clears a colour this reader cannot show, and the panel it would have sat on.
///
/// WHY the pair is asked together: a site written for two themes carries the ink
/// of one and the surface of the other, and the file that reaches a reader
/// painting light may be the dark one. `github.com`'s near-white
/// `--fgColor-default` landed on this engine's white page and whole paragraphs
/// disappeared -- the colour was in the stylesheet, was right on the screen it
/// was written for, and was invisible on the one it was drawn on.
///
/// WHY a failed ink leaves the text rather than losing it: the reader falls back
/// to its own colour, and the words are the point of a page while a shade is not.
///
/// WHY a panel is asked too: an ink dropped above leaves the reader's own, and a
/// panel dark enough that it cannot be written on is the same failure in
/// reverse. An author's dark panel survives when the same block carries light
/// ink, because then the pair answers for itself.
fn drop_unshown(style: &mut ComputedStyle) {
    if style.has_color && !ink_shows(style.color, band_surface(style)) {
        style.has_color = false;
        style.color = 0;
    }
    if style.has_background {
        let ink = if style.has_color { style.color } else { PAGE_INK };
        if !ink_shows(ink, style.background) {
            style.has_background = false;
            style.background = 0;
        }
    }
}

/// Parses a declaration list (`a: b; c: d`) onto `base`.
///
/// A colour is kept only while the surface it lands on can show it: see
/// [`drop_unshown`].
pub fn parse_declarations<'a>(
    bytes: &'a [u8],
    base: ComputedStyle,
    tokens: TokenStore<'a>,
) -> ComputedStyle {
    let mut style = base;
    let mut start = 0usize;
    let mut count = 0usize;
    while start < bytes.len() && count < MAX_INLINE_DECLARATIONS {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b';')
            .map(|p| start + p)
            .unwrap_or(bytes.len());
        let decl = &bytes[start..end];
        if decl.len() <= MAX_DECLARATION_LEN {
            if let Some(colon) = decl.iter().position(|&b| b == b':') {
                apply_declaration(&mut style, &decl[..colon], &decl[colon + 1..], tokens);
                count += 1;
            }
        }
        start = end.saturating_add(1);
    }
    drop_unshown(&mut style);
    style
}

fn parse_compound(bytes: &[u8]) -> Option<Compound<'_>> {
    let bytes = trim(bytes);
    if bytes.is_empty() || bytes.len() > MAX_SELECTOR_LEN {
        return None;
    }
    let mut compound = Compound::EMPTY;
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_') {
        i += 1;
    }
    if i > 0 {
        compound.tag = &bytes[..i];
    }
    while i < bytes.len() {
        let marker = bytes[i];
        if marker == b'.' || marker == b'#' {
            let name_start = i + 1;
            let mut j = name_start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_') {
                j += 1;
            }
            if j == name_start {
                return None;
            }
            if marker == b'.' {
                if !compound.class.is_empty() {
                    // More than one class in a compound is not implemented.
                    return None;
                }
                compound.class = &bytes[name_start..j];
            } else {
                if !compound.id.is_empty() {
                    return None;
                }
                compound.id = &bytes[name_start..j];
            }
            i = j;
        } else {
            // `>`, `+`, `~`, `:`, `::`, `[`, `*` and anything else this
            // subset does not implement make the whole selector unusable.
            return None;
        }
    }
    if compound.tag.is_empty() && compound.class.is_empty() && compound.id.is_empty() {
        return None;
    }
    Some(compound)
}

fn parse_selector(bytes: &[u8]) -> Option<Selector<'_>> {
    let bytes = trim(bytes);
    if bytes.is_empty() || bytes.len() > MAX_SELECTOR_LEN {
        return None;
    }
    let mut selector = Selector::EMPTY;
    let mut specificity = 0u16;
    let mut start = 0usize;
    while start < bytes.len() {
        while start < bytes.len() && is_selector_space(bytes[start]) {
            start += 1;
        }
        if start >= bytes.len() {
            break;
        }
        let mut end = start;
        while end < bytes.len() && !is_selector_space(bytes[end]) {
            end += 1;
        }
        if selector.count >= MAX_SELECTOR_PARTS {
            return None;
        }
        let compound = parse_compound(&bytes[start..end])?;
        if !compound.tag.is_empty() {
            specificity += 1;
        }
        if !compound.class.is_empty() {
            specificity += 10;
        }
        if !compound.id.is_empty() {
            specificity += 100;
        }
        selector.parts[selector.count] = compound;
        selector.count += 1;
        start = end;
    }
    if selector.count == 0 {
        return None;
    }
    selector.specificity = specificity;
    Some(selector)
}

fn is_selector_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r'
}

/// Where one rule sits inside the stylesheet text, as byte positions.
///
/// WHY positions and not slices: the same bytes are read by the parser, which
/// only looks, and by the compactor, which moves them. A borrow held across
/// the move would be a borrow of bytes being rewritten.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RulePos {
    selector_start: usize,
    selector_end: usize,
    body_start: usize,
    body_end: usize,
    /// Where the walk continues once this rule has been dealt with.
    next: usize,
}

/// The at-rule whose block holds rules the page means for the document.
const LAYER_NAME: &[u8] = b"layer";

/// Whether the at-rule beginning at `at` is a `@layer`.
///
/// WHY the letters after the word are asked: `@layered-things` is not a layer,
/// and treating its prelude as a selector would invent a rule the page never
/// wrote.
fn is_layer_at_rule(css: &[u8], limit: usize, at: usize) -> bool {
    if at + LAYER_NAME.len() > limit || !css[at..limit].starts_with(LAYER_NAME) {
        return false;
    }
    let after = at + LAYER_NAME.len();
    after >= limit
        || !(css[after].is_ascii_alphanumeric() || css[after] == b'-' || css[after] == b'_')
}

/// Finds the next rule a page means for the document, at or after `from`.
///
/// A comment contributes nothing. A `@layer` block is walked into, because it
/// states no condition -- its rules apply whenever the file does, and a bundled
/// site keeps the page's own colours inside one -- while a media query, a font
/// face or a keyframe set is skipped to its block's closing brace or to the
/// semicolon of a statement form, since nothing in this engine reads them. A
/// stray closing brace is stepped over rather than taken as the end of the
/// file: one unbalanced brace is still a stylesheet.
fn next_rule(css: &[u8], limit: usize, mut i: usize) -> Option<RulePos> {
    loop {
        if i >= limit {
            return None;
        }
        if css[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < limit && !css[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(limit);
            continue;
        }
        i = skip_ws(css, i);
        if i >= limit {
            return None;
        }
        if css[i] == b'@' {
            i += 1;
            let name = skip_ws(css, i);
            if is_layer_at_rule(css, limit, name) {
                // The prelude names the layer; its block holds rules the page
                // means for the document, so the walk goes through the brace
                // and reads them. A statement form (`@layer base, theme;`)
                // only orders layers and contributes nothing.
                i = name + LAYER_NAME.len();
                while i < limit && css[i] != b'{' && css[i] != b';' {
                    i += 1;
                }
                i = if i < limit && css[i] == b'{' {
                    i + 1
                } else {
                    (i + 1).min(limit)
                };
                continue;
            }
            while i < limit && css[i] != b'{' && css[i] != b';' {
                i += 1;
            }
            if i < limit && css[i] == b'{' {
                let mut depth = 1usize;
                i += 1;
                while i < limit && depth > 0 {
                    if css[i] == b'{' {
                        depth += 1;
                    } else if css[i] == b'}' {
                        depth -= 1;
                    }
                    i += 1;
                }
            } else if i < limit {
                i += 1;
            }
            continue;
        }
        let selector_start = i;
        while i < limit && css[i] != b'{' && css[i] != b'}' {
            i += 1;
        }
        if i >= limit {
            return None;
        }
        if css[i] == b'}' {
            i += 1;
            continue;
        }
        let selector_end = i;
        i += 1;
        let body_start = i;
        while i < limit && css[i] != b'}' {
            i += 1;
        }
        let body_end = i.min(limit);
        let next = if i < limit { i + 1 } else { i };
        return Some(RulePos {
            selector_start,
            selector_end,
            body_start,
            body_end,
            next,
        });
    }
}

/// Parses a stylesheet into a bounded rule list.
///
/// A rule that cannot be parsed is skipped; a comment, an at-rule and a
/// declaration that is not understood simply contribute nothing. The
/// scan reads at most [`MAX_CSS_BYTES`] and keeps at most [`MAX_RULES`]
/// rules. `tokens` is where a `var(--name)` finds the value the page wrote
/// for that name.
pub fn parse_stylesheet<'a>(css: &'a [u8], tokens: TokenStore<'a>) -> Stylesheet<'a> {
    let mut sheet = Stylesheet::empty();
    sheet.tokens = tokens;
    let limit = css.len().min(MAX_CSS_BYTES);
    let mut at = 0usize;
    while at < limit && sheet.len < MAX_RULES {
        let Some(pos) = next_rule(css, limit, at) else {
            break;
        };
        at = pos.next;
        let selector_text = &css[pos.selector_start..pos.selector_end];
        let body = &css[pos.body_start..pos.body_end];

        let style = parse_declarations(body, ComputedStyle::DEFAULT, tokens);
        if style.is_default() {
            continue;
        }
        // A selector list is split only as far as the rule and selector
        // caps; each selector then contributes the same declarations.
        let mut selector_at = 0usize;
        while selector_at < selector_text.len() && sheet.len < MAX_RULES {
            let comma = selector_text[selector_at..]
                .iter()
                .position(|&b| b == b',')
                .map(|p| selector_at + p)
                .unwrap_or(selector_text.len());
            if let Some(selector) = parse_selector(&selector_text[selector_at..comma]) {
                sheet.rules[sheet.len] = Rule { selector, style };
                sheet.len += 1;
            }
            selector_at = comma.saturating_add(1);
        }
    }
    sheet
}

/// Reports every `--token: value` a stylesheet declares, in the order the page
/// wrote them.
///
/// WHY the walk is its own: a file of colour tokens carries no rule this engine
/// paints from, so [`keep_readable_rules`] gives it back empty and the names it
/// defined would be lost with it. The rules that ask for those names live in
/// the files behind it, so the names are kept where the rules can reach them.
pub fn for_each_custom_property(css: &[u8], visit: &mut dyn FnMut(&[u8], &[u8])) {
    let limit = css.len().min(MAX_CSS_BYTES);
    let mut at = 0usize;
    while at < limit {
        let Some(pos) = next_rule(css, limit, at) else {
            break;
        };
        at = pos.next;
        let body = &css[pos.body_start..pos.body_end];
        let mut start = 0usize;
        while start < body.len() {
            let end = body[start..]
                .iter()
                .position(|&b| b == b';')
                .map(|p| start + p)
                .unwrap_or(body.len());
            let decl = &body[start..end];
            start = end.saturating_add(1);
            if decl.len() > MAX_DECLARATION_LEN {
                continue;
            }
            let Some(colon) = decl.iter().position(|&b| b == b':') else {
                continue;
            };
            let (property, value) = (trim(&decl[..colon]), trim(&decl[colon + 1..]));
            if let Some(name) = custom_property(property) {
                if !value.is_empty() && paintable(value) {
                    visit(name, value);
                }
            }
        }
    }
}

/// Whether a selector list could reach an element of this document.
///
/// WHY every part of a selector is asked and not only its last: `.cm-keyword a`
/// colours nothing unless the page carries `.cm-keyword`, and a bundled site
/// writes its utility sheets for markup a reader's page does not contain --
/// github.com's front door spent its whole rule budget on `.pl-*` and `.cm-*`
/// names that appear nowhere in the document, so the file holding the page's own
/// colours never reached it.
///
/// WHY a name that merely might be there is enough: this is a filter for
/// keeping, and holding a rule that turns out to match nothing costs a little
/// room while dropping one that colours the page costs the page.
fn selector_reaches(css: &[u8], start: usize, end: usize, names: NameSet<'_>) -> bool {
    let text = &css[start..end];
    let mut at = 0usize;
    while at < text.len() {
        let comma = text[at..]
            .iter()
            .position(|&b| b == b',')
            .map(|p| at + p)
            .unwrap_or(text.len());
        if let Some(selector) = parse_selector(&text[at..comma]) {
            let mut reaches = true;
            let mut part = 0usize;
            while part < selector.count && reaches {
                let compound = selector.parts[part];
                reaches = names.holds(NameKind::Tag, compound.tag)
                    && names.holds(NameKind::Class, compound.class)
                    && names.holds(NameKind::Id, compound.id);
                part += 1;
            }
            if reaches {
                return true;
            }
        }
        at = comma.saturating_add(1);
    }
    false
}

/// Rewrites a stylesheet, in place, to the rules this engine can apply.
///
/// WHY a file is cut down before the document keeps it: a bundled site leads
/// its head with colour-token sheets and one of them is bigger than the room a
/// page has for style text. Keeping whole text spent that room on declarations
/// the reader ignores and starved the file behind them that carries the page's
/// own colours and type.
///
/// WHY a rule is moved whole rather than once per selector: `a,b{color:red}`
/// names both elements either way, and writing a copy per name would make the
/// kept text larger than the bytes it came from -- which, in a buffer being
/// rewritten in place, would run over the rules the walk had not reached. As
/// written, the bytes placed are always a subset of the bytes walked, so the
/// write cursor never overtakes the read cursor.
///
/// WHY the declarations are copied verbatim: what the reader does not
/// understand it drops while parsing, so the text that reaches the parser has
/// to be the text the page wrote.
///
/// WHY a rule whose selector reaches nothing is not kept: the budget is spent
/// by the first files that carry readable rules, and on a bundled site those are
/// the utility sheets -- a rule budget of ninety-six was filled by two of them
/// while the page's own colours waited behind. A name the document never used
/// cannot be coloured by anything, so the room it would take goes to a rule that
/// can.
///
/// WHY an unshowable colour leaves a rule out as well: [`drop_unshown`] has
/// already taken the ink away, so nothing in the rule changes a pixel the reader
/// would otherwise paint, and keeping the text of it would charge the room to a
/// rule that cannot pay for it.
///
/// Returns how many bytes the kept rules occupy and how many rules they are.
pub fn keep_readable_rules(
    css: &mut [u8],
    max_rules: usize,
    tokens: TokenStore<'_>,
    names: NameSet<'_>,
) -> (usize, usize) {
    let limit = css.len().min(MAX_CSS_BYTES);
    let mut read = 0usize;
    let mut written = 0usize;
    let mut kept = 0usize;
    while kept < max_rules {
        let Some(pos) = next_rule(css, limit, read) else {
            break;
        };
        let style =
            parse_declarations(&css[pos.body_start..pos.body_end], ComputedStyle::DEFAULT, tokens);
        let carries = !style.is_default()
            && selector_reaches(css, pos.selector_start, pos.selector_end, names);
        read = pos.next;
        if !carries {
            continue;
        }
        let mut source = pos.selector_start;
        while source < pos.selector_end {
            css[written] = css[source];
            written += 1;
            source += 1;
        }
        css[written] = b'{';
        written += 1;
        let mut source = pos.body_start;
        while source < pos.body_end {
            css[written] = css[source];
            written += 1;
            source += 1;
        }
        css[written] = b'}';
        written += 1;
        kept += 1;
    }
    (written, kept)
}

fn class_contains(attr: &[u8], wanted: &[u8]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    let mut start = 0usize;
    while start <= attr.len() {
        let end = attr[start..]
            .iter()
            .position(|&b| is_selector_space(b))
            .map(|p| start + p)
            .unwrap_or(attr.len());
        if attr[start..end].eq_ignore_ascii_case(wanted) {
            return true;
        }
        start = end.saturating_add(1);
        if start > attr.len() {
            break;
        }
    }
    false
}

/// Whether one compound matches one element.
pub fn compound_matches(compound: &Compound<'_>, element: &Compound<'_>) -> bool {
    if !compound.tag.is_empty() && !eq_ignore_ascii(compound.tag, element.tag) {
        return false;
    }
    if !compound.class.is_empty() && !class_contains(element.class, compound.class) {
        return false;
    }
    if !compound.id.is_empty() && !eq_ignore_ascii(compound.id, element.id) {
        return false;
    }
    true
}

/// Whether a selector matches an element with the given ancestors.
///
/// `ancestors` is ordered outermost first. Descendant matching is
/// greedy right-to-left, which is exact for descendant combinators.
pub fn selector_matches(selector: &Selector<'_>, element: &Compound<'_>, ancestors: &[Compound<'_>]) -> bool {
    if selector.count == 0 {
        return false;
    }
    let subject = &selector.parts[selector.count - 1];
    if !compound_matches(subject, element) {
        return false;
    }
    let mut ancestor_index = ancestors.len();
    let mut part = selector.count - 1;
    while part > 0 {
        part -= 1;
        let wanted = &selector.parts[part];
        let mut found = false;
        while ancestor_index > 0 {
            ancestor_index -= 1;
            if compound_matches(wanted, &ancestors[ancestor_index]) {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

fn merge(style: &mut ComputedStyle, specs: &mut Specs, rule: &ComputedStyle, specificity: u16) {
    if rule.has_color && specificity >= specs.color {
        style.color = rule.color;
        style.has_color = true;
        specs.color = specificity;
    }
    if rule.has_background && specificity >= specs.background {
        style.background = rule.background;
        style.has_background = true;
        specs.background = specificity;
    }
    if rule.font_scale != 0 && specificity >= specs.font {
        style.font_scale = rule.font_scale;
        specs.font = specificity;
    }
    if rule.margin_top != 0 && specificity >= specs.margin {
        style.margin_top = rule.margin_top;
        specs.margin = specificity;
    }
    if rule.padding_left != 0 && specificity >= specs.padding {
        style.padding_left = rule.padding_left;
        specs.padding = specificity;
    }
    if rule.has_border && specificity >= specs.border {
        style.border_color = rule.border_color;
        style.has_border = true;
        specs.border = specificity;
    }
}

/// The cascade for one element: matching rules in specificity order, then
/// the inline `style` attribute, which always wins (short of `!important`,
/// which this subset does not implement).
pub fn cascade(
    sheet: &Stylesheet<'_>,
    element: Compound<'_>,
    ancestors: &[Compound<'_>],
    inline: Option<&[u8]>,
) -> ComputedStyle {
    let mut style = ComputedStyle::DEFAULT;
    let mut specs = Specs::NONE;
    for rule in &sheet.rules[..sheet.len.min(MAX_RULES)] {
        if selector_matches(&rule.selector, &element, ancestors) {
            merge(&mut style, &mut specs, &rule.style, rule.selector.specificity);
        }
    }
    if let Some(inline) = inline {
        let inline_style = parse_declarations(inline, ComputedStyle::DEFAULT, sheet.tokens);
        merge(&mut style, &mut specs, &inline_style, SPEC_INLINE);
    }
    style
}

/// The sentinel that opens a style marker line in a reader document.
pub const STYLE_MARKER: u8 = 0x01;

/// Whether a reader-document line is a style marker.
pub fn is_style_marker(line: &[u8]) -> bool {
    line.len() >= 3 && line[0] == STYLE_MARKER && line[1] == b'S'
}

/// Encodes a computed style as one marker line (no trailing newline).
///
/// The format is fixed-width so parsing cannot run past the marker and
/// a malformed marker is simply refused:
/// `0x01 'S' cccccc bbbbbb f mm pp rrrrrr 0x02`.
pub fn encode_style_marker(style: &ComputedStyle, out: &mut [u8]) -> usize {
    const LEN: usize = 26;
    if out.len() < LEN {
        return 0;
    }
    out[0] = STYLE_MARKER;
    out[1] = b'S';
    write_hex6(&mut out[2..8], if style.has_color { style.color } else { 0 }, style.has_color);
    write_hex6(
        &mut out[8..14],
        if style.has_background { style.background } else { 0 },
        style.has_background,
    );
    out[14] = b'0' + style.font_scale.min(9);
    out[15] = b'0' + (style.margin_top / 10).min(9);
    out[16] = b'0' + (style.margin_top % 10);
    out[17] = b'0' + (style.padding_left / 10).min(9);
    out[18] = b'0' + (style.padding_left % 10);
    write_hex6(
        &mut out[19..25],
        if style.has_border { style.border_color } else { 0 },
        style.has_border,
    );
    out[25] = 0x02;
    LEN
}

fn write_hex6(out: &mut [u8], value: u32, present: bool) {
    if !present {
        for b in out.iter_mut() {
            *b = b'-';
        }
        return;
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for i in 0..6 {
        out[i] = HEX[((value >> ((5 - i) * 4)) & 0xF) as usize];
    }
}

fn read_hex6(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 6 || bytes.iter().any(|&b| b == b'-') {
        return None;
    }
    let mut value = 0u32;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a' + 10) as u32,
            b'A'..=b'F' => (b - b'A' + 10) as u32,
            _ => return None,
        };
        value = (value << 4) | digit;
    }
    Some(0xFF000000 | value)
}

fn read_marker_hex6(bytes: &[u8]) -> Option<Option<u32>> {
    if bytes.len() != 6 {
        return None;
    }
    if bytes == b"------" {
        return Some(None);
    }
    read_hex6(bytes).map(Some)
}

/// Parses one style marker line. A malformed marker yields `None`.
pub fn parse_style_marker(line: &[u8]) -> Option<ComputedStyle> {
    if !is_style_marker(line) || line.len() != 26 || line[25] != 0x02 {
        return None;
    }
    let color = read_marker_hex6(&line[2..8])?;
    let background = read_marker_hex6(&line[8..14])?;
    if !line[14].is_ascii_digit()
        || !line[15].is_ascii_digit()
        || !line[16].is_ascii_digit()
        || !line[17].is_ascii_digit()
        || !line[18].is_ascii_digit()
    {
        return None;
    }
    let font_scale = line[14] - b'0';
    if font_scale > 2 {
        return None;
    }
    let margin_top = ((line[15] - b'0') as u16) * 10 + (line[16] - b'0') as u16;
    let padding_left = ((line[17] - b'0') as u16) * 10 + (line[18] - b'0') as u16;
    let border = read_marker_hex6(&line[19..25])?;
    Some(ComputedStyle {
        color: color.unwrap_or(0),
        has_color: color.is_some(),
        background: background.unwrap_or(0),
        has_background: background.is_some(),
        font_scale,
        margin_top: margin_top.min(32) as u8,
        padding_left: padding_left.min(32) as u8,
        border_color: border.unwrap_or(0),
        has_border: border.is_some(),
    })
}

/// The number of rules a sheet holds.
pub fn rule_count(sheet: &Stylesheet<'_>) -> usize {
    sheet.len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element<'a>(tag: &'a [u8], class: &'a [u8], id: &'a [u8]) -> Compound<'a> {
        Compound { tag, class, id }
    }

    #[test]
    fn a_style_rule_changes_the_computed_color() {
        let sheet = parse_stylesheet(b"p { color: #ff0000; }", TokenStore::NONE);
        assert_eq!(rule_count(&sheet), 1);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert!(style.has_color);
        assert_eq!(style.color, 0xFFFF0000);
    }

    #[test]
    fn the_inline_style_wins_over_a_tag_rule() {
        let sheet = parse_stylesheet(b"p { color: #0a7d0a; margin: 8px; }", TokenStore::NONE);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], Some(b"color: #0000ff"));
        assert_eq!(style.color, 0xFF0000FF);
        // A property the inline attribute does not name still comes from
        // the rule.
        assert_eq!(style.margin_top, 8);
    }

    #[test]
    fn unknown_properties_are_ignored() {
        let sheet = parse_stylesheet(b"p { bogus-property: 3px; color: #123456; }", TokenStore::NONE);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF123456);
        assert_eq!(style.background, 0);
        assert_eq!(style.margin_top, 0);
        assert_eq!(style.font_scale, 0);
    }

    #[test]
    fn malformed_rules_do_not_panic() {
        let cases: [&[u8]; 10] = [
            b"",
            b"}",
            b"p",
            b"p {",
            b"p { color: }",
            b"p { color: #12",
            b"p { color: #123456",
            b"@media screen { p { color: red } }",
            b"p { color red; }",
            b". { color: #fff }",
        ];
        for case in cases {
            let sheet = parse_stylesheet(case, TokenStore::NONE);
            let _ = cascade(&sheet, element(b"p", b"", b""), &[], Some(case));
            assert!(sheet.len <= MAX_RULES);
        }
    }

    #[test]
    fn descendant_selectors_match_the_named_ancestor() {
        let sheet = parse_stylesheet(b".card p { color: #ff8800; }", TokenStore::NONE);
        let ancestors = [element(b"div", b"card", b"")];
        let matched = cascade(&sheet, element(b"p", b"", b""), &ancestors, None);
        assert_eq!(matched.color, 0xFFFF8800);

        let other_ancestors = [element(b"div", b"plain", b"")];
        let unmatched = cascade(&sheet, element(b"p", b"", b""), &other_ancestors, None);
        assert!(!unmatched.has_color);
    }

    #[test]
    fn specificity_orders_id_then_class_then_tag() {
        let sheet = parse_stylesheet(
            b"p { color: #111111; } .note { color: #222222; } #lead { color: #333333; }",
            TokenStore::NONE,
        );
        let style = cascade(
            &sheet,
            element(b"p", b"note", b"lead"),
            &[],
            None,
        );
        assert_eq!(style.color, 0xFF333333);
        let class_only = cascade(&sheet, element(b"p", b"note", b""), &[], None);
        assert_eq!(class_only.color, 0xFF222222);
        let tag_only = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert_eq!(tag_only.color, 0xFF111111);
    }

    #[test]
    fn a_later_rule_of_equal_specificity_wins() {
        let sheet = parse_stylesheet(b"p { color: #111111; } p { color: #222222; }", TokenStore::NONE);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF222222);
    }

    #[test]
    fn font_size_lands_on_the_two_platform_rungs() {
        assert_eq!(font_scale_for(b"12px"), Some(1));
        assert_eq!(font_scale_for(b"18px"), Some(2));
        assert_eq!(font_scale_for(b"24"), Some(2));
        assert_eq!(font_scale_for(b"small"), Some(1));
        assert_eq!(font_scale_for(b"3em"), None);
        let sheet = parse_stylesheet(b"p { font-size: 24px; }", TokenStore::NONE);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert_eq!(style.font_scale, 2);
    }

    #[test]
    fn margins_padding_and_border_are_parsed() {
        let sheet = parse_stylesheet(
            b"div { margin: 12px; padding: 10px 4px; border: 1px solid #334455; }",
            TokenStore::NONE,
        );
        let style = cascade(&sheet, element(b"div", b"", b""), &[], None);
        assert_eq!(style.margin_top, 12);
        // `padding: 10px 4px` is vertical 10 and horizontal 4 in CSS.
        assert_eq!(style.padding_left, 4);
        assert!(style.has_border);
        assert_eq!(style.border_color, 0xFF334455);
    }

    #[test]
    fn unsupported_selectors_are_skipped() {
        let sheet = parse_stylesheet(
            b"div > p { color: #ff0000; } a:hover { color: #ff0000; } * { color: #ff0000; }",
            TokenStore::NONE,
        );
        assert_eq!(rule_count(&sheet), 0);
    }

    #[test]
    fn the_rule_and_selector_caps_hold() {
        let mut css = [0u8; 4096];
        let mut w = 0usize;
        for i in 0..80 {
            let rule = b"p { color: #123456; }\n";
            if w + rule.len() > css.len() {
                break;
            }
            css[w..w + rule.len()].copy_from_slice(rule);
            w += rule.len();
            let _ = i;
        }
        let sheet = parse_stylesheet(&css[..w], TokenStore::NONE);
        assert!(sheet.len <= MAX_RULES);
        let long_selector = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa { color: #fff }";
        let long_sheet = parse_stylesheet(long_selector, TokenStore::NONE);
        assert_eq!(long_sheet.len, 0);
    }

    #[test]
    fn a_style_marker_round_trips() {
        let style = ComputedStyle {
            color: 0xFF112233,
            has_color: true,
            background: 0xFF445566,
            has_background: true,
            font_scale: 2,
            margin_top: 14,
            padding_left: 9,
            border_color: 0xFF778899,
            has_border: true,
        };
        let mut buf = [0u8; 32];
        let n = encode_style_marker(&style, &mut buf);
        assert!(is_style_marker(&buf[..n]));
        assert_eq!(parse_style_marker(&buf[..n]), Some(style));

        let plain = ComputedStyle::DEFAULT;
        let n = encode_style_marker(&plain, &mut buf);
        assert_eq!(parse_style_marker(&buf[..n]), Some(ComputedStyle::DEFAULT));

        assert_eq!(parse_style_marker(b"\x01Sgarbage"), None);
        assert_eq!(parse_style_marker(b"ordinary text"), None);
        // Malformed hex or mixed dashes must be rejected
        assert_eq!(parse_style_marker(b"\x01S------12345z00000------\x02"), None);
        assert_eq!(parse_style_marker(b"\x01S---123------00000------\x02"), None);
        assert_eq!(parse_style_marker(b"\x01S1122334455660a000778899\x02"), None);
    }

    #[test]
    fn stray_closing_brace_does_not_abort_parsing() {
        let css = b"div { color: #ff0000; } } p { color: #0000ff; }";
        let sheet = parse_stylesheet(css, TokenStore::NONE);
        assert_eq!(rule_count(&sheet), 2);
    }

    #[test]
    fn oversized_declaration_is_dropped() {
        let mut long_decl = [b'a'; 120];
        long_decl[..12].copy_from_slice(b"color: #fff;");
        let mut css = [0u8; 200];
        let prefix = b"p { ";
        let suffix = b"; font-size: large; }";
        css[..prefix.len()].copy_from_slice(prefix);
        let mut pos = prefix.len();
        css[pos..pos + long_decl.len()].copy_from_slice(&long_decl);
        pos += long_decl.len();
        css[pos..pos + suffix.len()].copy_from_slice(suffix);
        pos += suffix.len();

        let sheet = parse_stylesheet(&css[..pos], TokenStore::NONE);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        // The oversized declaration should be dropped, but the subsequent valid one parsed
        assert_eq!(style.font_scale, 2);
    }

    #[test]
    fn a_file_is_cut_down_to_the_rules_the_reader_can_apply() {
        let whole_text = b":root{--color-accent-fg:#0969da;color-scheme:light}\
@media (prefers-color-scheme:dark){:root{color:red}}/* tokens */\
a{color:#0969da;text-decoration:underline}h1{font-size:large;margin-top:24px}";
        let mut css = [0u8; 256];
        css[..whole_text.len()].copy_from_slice(whole_text);
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 2);
        assert_eq!(
            &css[..written],
            b"a{color:#0969da;text-decoration:underline}h1{font-size:large;margin-top:24px}"
        );
        // What the kept text says is what the whole file said, for the rules
        // this engine reads.
        let compacted = parse_stylesheet(&css[..written], TokenStore::NONE);
        let whole = parse_stylesheet(whole_text, TokenStore::NONE);
        assert_eq!(compacted.len, whole.len);
        assert_eq!(compacted.rules[0].style, whole.rules[0].style);
        assert_eq!(compacted.rules[1].style, whole.rules[1].style);
    }

    #[test]
    fn a_rule_naming_two_elements_is_kept_once() {
        // WHY the list is not split: one copy per name would write more bytes
        // than the rule occupied, and the extra would land on rules the walk
        // had not reached yet.
        let mut css = *b"a,b{color:#ff0000}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"a,b{color:#ff0000}");
        assert_eq!(parse_stylesheet(&css[..written], TokenStore::NONE).len, 2);
    }

    #[test]
    fn a_token_sheet_keeps_nothing_and_holds_no_room() {
        let mut css = *b":root{--color-canvas-default:#ffffff;--color-fg-default:#1f2328}";
        assert_eq!(keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE), (0, 0));
    }

    #[test]
    fn compaction_stops_where_the_rule_budget_is_spent() {
        let mut css = *b"p{color:#ff0000}a{color:#0a7d0a}h1{color:#0000ff}";
        let (written, kept) = keep_readable_rules(&mut css, 2, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 2);
        assert_eq!(&css[..written], b"p{color:#ff0000}a{color:#0a7d0a}");
    }

    #[test]
    fn a_rule_behind_a_wall_of_tokens_survives_the_move() {
        let mut css = [b' '; 600];
        let head = b":root{--color-accent-fg:";
        css[..head.len()].copy_from_slice(head);
        let rule = b";}a{color:#0969da}";
        css[560..560 + rule.len()].copy_from_slice(rule);
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"a{color:#0969da}");
        let sheet = parse_stylesheet(&css[..written], TokenStore::NONE);
        let style = cascade(&sheet, element(b"a", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF0969DA);
    }

    /// The names and values the tests below answer from, laid end to end the
    /// way the document's token arena lays them: `--a` and `--b` name a colour,
    /// and `--c` names `--a` the way a theme file names another theme's colour.
    fn theme() -> TokenStore<'static> {
        const TEXT: &[u8] = b"--a#111111--b#222222--cvar(--a)";
        const SLOTS: &[TokenSlot] = &[
            TokenSlot { name: (0, 3), value: (3, 7) },
            TokenSlot { name: (10, 3), value: (13, 7) },
            TokenSlot { name: (20, 3), value: (23, 8) },
        ];
        TokenStore { slots: SLOTS, text: TEXT }
    }

    #[test]
    fn a_var_is_answered_by_the_value_its_name_was_given() {
        let tokens = theme();
        assert_eq!(resolve_custom(tokens, b"var(--a)"), b"#111111");
        assert_eq!(resolve_custom(tokens, b"  var(--b)  "), b"#222222");
    }

    #[test]
    fn a_name_defined_after_another_is_walked_to_the_colour() {
        let tokens = theme();
        assert_eq!(resolve_custom(tokens, b"var(--c)"), b"#111111");
    }

    #[test]
    fn an_unknown_name_keeps_the_fallback_the_rule_offered() {
        let tokens = theme();
        assert_eq!(resolve_custom(tokens, b"var(--zz, #0969da)"), b"#0969da");
        // With no fallback there is nothing to say, and the declaration is
        // refused for what it is rather than painted with a guessed colour.
        assert!(resolve_custom(tokens, b"var(--zz)").is_empty());
    }

    #[test]
    fn a_name_defined_in_terms_of_itself_stops_answering() {
        const TEXT: &[u8] = b"--avar(--a)";
        let tokens = TokenStore {
            slots: &[TokenSlot { name: (0, 3), value: (3, 8) }],
            text: TEXT,
        };
        // The walk has a hop budget, so a page that names a colour after
        // itself gets the question back instead of a loop.
        assert_eq!(resolve_custom(tokens, b"var(--a)"), b"var(--a)");
    }

    #[test]
    fn a_colour_asked_of_a_token_reaches_the_rule_that_kept_it() {
        // The two files a bundled site splits its design across: the one that
        // names the colours and the one that paints with them.
        let tokens = theme();
        let whole_text = b":root{--a:#111111}a{color:var(--a)}p{color:#000000}";
        let mut css = [0u8; 64];
        css[..whole_text.len()].copy_from_slice(whole_text);
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, tokens, NameSet::NONE);
        assert_eq!(kept, 2);
        assert_eq!(&css[..written], b"a{color:var(--a)}p{color:#000000}");
        let sheet = parse_stylesheet(&css[..written], tokens);
        let style = cascade(&sheet, element(b"a", b"", b""), &[], None);
        assert!(style.has_color);
        assert_eq!(style.color, 0xFF111111);
    }

    #[test]
    fn a_colour_asked_of_a_name_nobody_declared_is_not_painted() {
        let mut css = *b"a{color:var(--missing)}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!((written, kept), (0, 0));
    }

    #[test]
    fn a_custom_property_is_not_mistaken_for_a_painted_thing() {
        let tokens = theme();
        let sheet = parse_stylesheet(b":root{--a:#111111}", tokens);
        assert_eq!(sheet.len, 0);
        let mut style = ComputedStyle::DEFAULT;
        apply_declaration(&mut style, b"--a", b"#111111", tokens);
        apply_declaration(&mut style, b"color", b"var(--a)", tokens);
        assert!(style.has_color);
        assert_eq!(style.color, 0xFF111111);
    }

    #[test]
    fn an_inline_style_asks_the_sheet_for_its_names() {
        let tokens = theme();
        let sheet = parse_stylesheet(b"a { color: #222222; }", tokens);
        let style = cascade(&sheet, element(b"a", b"", b""), &[], Some(b"color: var(--a)"));
        assert_eq!(style.color, 0xFF111111);
    }

    /// The bits a document carrying only these names would answer with.
    fn document_names(tags: &[&[u8]], classes: &[&[u8]], ids: &[&[u8]]) -> [u64; NAME_WORDS] {
        let mut bits = [0u64; NAME_WORDS];
        for tag in tags {
            mark_name(&mut bits, NameKind::Tag, tag);
        }
        for class in classes {
            mark_name(&mut bits, NameKind::Class, class);
        }
        for id in ids {
            mark_name(&mut bits, NameKind::Id, id);
        }
        bits
    }

    #[test]
    fn a_rule_for_a_class_the_document_never_used_is_not_kept() {
        let bits = document_names(&[b"div"], &[b"card"], &[]);
        let names = NameSet { bits: &bits, known: true };
        let mut css = *b".cm-keyword{color:#aa0000}.card{color:#00aa00}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, names);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b".card{color:#00aa00}");
    }

    #[test]
    fn a_name_the_document_holds_is_answered_whatever_way_it_is_written() {
        let bits = document_names(&[b"div"], &[b"Hero"], &[]);
        let names = NameSet { bits: &bits, known: true };
        // The reader matches without regard to case, so a page that writes its
        // markup `<DIV CLASS="hero">` and its style `.HERO div` is the same page.
        let mut css = *b"DIV{color:#0000ff}.HERO{color:#ff0000}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, names);
        assert_eq!(kept, 2);
        assert_eq!(&css[..written], b"DIV{color:#0000ff}.HERO{color:#ff0000}");
    }

    #[test]
    fn a_rule_needs_every_name_it_asks_for_to_be_in_the_page() {
        // `.card a` colours no link unless the page carries a `.card`, so a
        // sheet written for markup this document does not contain contributes
        // nothing -- while a name the document does hold keeps its rule.
        let bits = document_names(&[b"a", b"nav"], &[], &[]);
        let names = NameSet { bits: &bits, known: true };
        let mut css = *b".card a{color:#aa0000}nav a{color:#00aa00}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, names);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"nav a{color:#00aa00}");
    }

    #[test]
    fn a_utility_sheet_does_not_spend_the_room_the_page_needs() {
        // The shape that cost github.com its colours: a file of names the
        // document never used arrived first and took the whole budget.
        let bits = document_names(&[b"a"], &[b"Link"], &[]);
        let names = NameSet { bits: &bits, known: true };
        let mut utility = *b".pl-c{color:#999999}.pl-s{color:#0969da}.cm-keyword{color:#aa0000}";
        assert_eq!(keep_readable_rules(&mut utility, 2, TokenStore::NONE, names), (0, 0));

        let mut own = *b"a{color:#0969da}.Link{color:#0969da}";
        let (written, kept) = keep_readable_rules(&mut own, 2, TokenStore::NONE, names);
        assert_eq!(kept, 2);
        assert_eq!(&own[..written], b"a{color:#0969da}.Link{color:#0969da}");
    }

    #[test]
    fn a_rule_inside_a_layer_is_read_because_the_layer_asks_nothing() {
        // A bundled site writes `@layer base { a { color: … } }` to keep its own
        // rules ahead of the page's, and the block states no condition: the rule
        // applies whenever the file does.
        let sheet = parse_stylesheet(b"@layer primer-css-base{a{color:#0969da}}", TokenStore::NONE);
        assert_eq!(sheet.len, 1);
        let style = cascade(&sheet, element(b"a", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF0969DA);
        let mut css = *b"@layer base{a{color:#0969da}}@layer theme{p{color:#111111}}";
        let (written, kept) =
            keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 2);
        assert_eq!(&css[..written], b"a{color:#0969da}p{color:#111111}");
    }

    #[test]
    fn a_layer_statement_and_a_rule_behind_a_media_query_are_still_skipped() {
        // `@layer a, b;` only orders layers, and a media query is a condition
        // this reader cannot evaluate, so neither contributes a rule.
        let mut css = *b"@layer base, theme;@media (prefers-color-scheme:dark){a{color:#ffffff}}h1{color:#111111}";
        let (written, kept) =
            keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"h1{color:#111111}");
    }

    #[test]
    fn an_at_rule_whose_name_only_begins_like_layer_is_not_walked_into() {
        let sheet = parse_stylesheet(b"@layered-things{a{color:#0969da}}p{color:#111111}", TokenStore::NONE);
        assert_eq!(sheet.len, 1);
        let style = cascade(&sheet, element(b"p", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF111111);
    }

    #[test]
    fn a_document_nobody_read_keeps_every_readable_rule() {
        // WHY: a caller with no tree in hand is only asking whether a selector
        // is readable, and refusing every name it never looked at would drop
        // the rules of a page it could still paint.
        let mut css = *b".whatever{color:#0969da}";
        let (written, kept) = keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b".whatever{color:#0969da}");
    }

    #[test]
    fn an_ink_the_surface_cannot_show_is_left_to_the_reader() {
        // The failure a dark theme's file brings to a page this engine paints
        // light: the ink is the stylesheet's own and is near the colour of the
        // surface, so the words it colours go missing with the colour.
        let sheet = parse_stylesheet(b"body{color:#f0f6fc}", TokenStore::NONE);
        assert_eq!(sheet.len, 0);
        let style = cascade(&sheet, element(b"body", b"", b""), &[], None);
        assert!(!style.has_color);
    }

    #[test]
    fn a_colour_the_page_wrote_for_this_surface_still_colours() {
        // github.com's own link blue on a white page: enough apart to read, and
        // the whole point of keeping the page's rules.
        let sheet = parse_stylesheet(b"a{color:#0969da}", TokenStore::NONE);
        assert_eq!(sheet.len, 1);
        let style = cascade(&sheet, element(b"a", b"", b""), &[], None);
        assert_eq!(style.color, 0xFF0969DA);
        // A shade a page uses for secondary text is dim by intent and stays.
        let muted = parse_stylesheet(b"p{color:#8b949e}", TokenStore::NONE);
        assert_eq!(muted.len, 1);
    }

    #[test]
    fn a_light_ink_on_the_dark_panel_that_names_it_is_kept() {
        // The pair answers for itself: the rule states both halves, and the
        // reader paints the panel it was written against.
        let sheet = parse_stylesheet(b".btn{color:#ffffff;background:#0969da}", TokenStore::NONE);
        let style = cascade(&sheet, element(b"div", b"btn", b""), &[], None);
        assert!(style.has_color && style.has_background);
        assert_eq!(style.color, 0xFFFFFFFF);
        assert_eq!(style.background, 0xFF0969DA);
    }

    #[test]
    fn a_panel_the_readers_own_ink_cannot_be_written_on_is_not_painted() {
        // The same failure in reverse: a dark theme's body background, with the
        // reader's dark ink on it, is a page of unreadable words.
        let mut css = *b"body{background:#0d1117}";
        let (written, kept) =
            keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!((written, kept), (0, 0));
    }

    #[test]
    fn an_unshowable_colour_costs_no_room() {
        // The utility sheet of near-white inks is cut out of the budget, and the
        // rule behind it that this surface can show is the one kept.
        let mut css = *b"a{color:#f0f6fc}p{color:#0969da}";
        let (written, kept) =
            keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"p{color:#0969da}");
    }

    #[test]
    fn a_rule_keeping_its_padding_keeps_its_text_too() {
        // Only the ink is refused: what the rule still changes is a pixel the
        // reader does paint, so the bytes stay.
        let mut css = *b"p{color:#f0f6fc;padding-left:4px}";
        let (written, kept) =
            keep_readable_rules(&mut css, MAX_RULES, TokenStore::NONE, NameSet::NONE);
        assert_eq!(kept, 1);
        assert_eq!(&css[..written], b"p{color:#f0f6fc;padding-left:4px}");
    }
}
