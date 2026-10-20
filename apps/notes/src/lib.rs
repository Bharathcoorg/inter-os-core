//! InterOS Sovereign Native Notes Library
//!
//! Provides core data structures, storage serialization, and text editing
//! buffer engine for the native Ring-3 Notes application.
//!
//! Enforces:
//! - `#![no_std]` compliance with zero heap allocations
//! - Fletcher-32 binary serialization with checksum integrity
//! - Deterministic word-wrapping and multiline layout calculation
//! - Sovereign storage lifecycle and platform-truth guarantees

#![no_std]

pub use interos_std as std;

pub const MAX_NOTES: usize = 16;
pub const MAX_TITLE_LEN: usize = 48;
pub const MAX_BODY_LEN: usize = 1024;
pub const MAGIC_HEADER: [u8; 6] = *b"NOTE\x02\x00";

// =============================================================================
// Google Keep-Style Color Palettes & Categories
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteColorTheme {
    pub bg: u32,
    pub border: u32,
    pub chip_bg: u32,
    pub accent: u32,
    pub text_primary: u32,
    pub text_body: u32,
    pub text_muted: u32,
}

pub const KEEP_COLORS: [NoteColorTheme; 8] = [
    // 0: Slate / Obsidian (Clean Dark)
    NoteColorTheme {
        bg: 0xFF1C1F26,
        border: 0xFF2D323E,
        chip_bg: 0xFF282D38,
        accent: 0xFF94A3B8,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFCBD5E1,
        text_muted: 0xFF64748B,
    },
    // 1: Coral / Warm Terracotta
    NoteColorTheme {
        bg: 0xFF3D2020,
        border: 0xFF5C2B2B,
        chip_bg: 0xFF4D2828,
        accent: 0xFFF87171,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFFECACA,
        text_muted: 0xFFFCA5A5,
    },
    // 2: Warm Honey / Amber
    NoteColorTheme {
        bg: 0xFF382A14,
        border: 0xFF543E1B,
        chip_bg: 0xFF483518,
        accent: 0xFFFBBF24,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFFDE68A,
        text_muted: 0xFFFCD34D,
    },
    // 3: Fresh Sage / Mint
    NoteColorTheme {
        bg: 0xFF163324,
        border: 0xFF224E37,
        chip_bg: 0xFF1D422E,
        accent: 0xFF34D399,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFA7F3D0,
        text_muted: 0xFF6EE7B7,
    },
    // 4: Ocean Teal
    NoteColorTheme {
        bg: 0xFF143036,
        border: 0xFF1E4852,
        chip_bg: 0xFF1A3E46,
        accent: 0xFF2DD4BF,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFF99F6E4,
        text_muted: 0xFF5EEAD4,
    },
    // 5: Dusk Sky / Cerulean
    NoteColorTheme {
        bg: 0xFF16263B,
        border: 0xFF223A5B,
        chip_bg: 0xFF1C314B,
        accent: 0xFF60A5FA,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFBFDBFE,
        text_muted: 0xFF93C5FD,
    },
    // 6: Royal Orchid / Lavender
    NoteColorTheme {
        bg: 0xFF2B1A3B,
        border: 0xFF402758,
        chip_bg: 0xFF36204B,
        accent: 0xFFC084FC,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFE9D5FF,
        text_muted: 0xFFD8B4FE,
    },
    // 7: Blossom / Rose
    NoteColorTheme {
        bg: 0xFF381829,
        border: 0xFF54243E,
        chip_bg: 0xFF461E34,
        accent: 0xFFF472B6,
        text_primary: 0xFFFFFFFF,
        text_body: 0xFFFBCFE8,
        text_muted: 0xFFF9A8D4,
    },
];

pub const CATEGORY_NAMES: [&[u8]; 5] = [
    b"All",
    b"Work",
    b"Personal",
    b"Ideas",
    b"System",
];

// =============================================================================
// Note Data Structure
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: u32,
    pub title: [u8; MAX_TITLE_LEN],
    pub title_len: usize,
    pub body: [u8; MAX_BODY_LEN],
    pub body_len: usize,
    pub created_at: u64,
    pub updated_at: u64,
    pub is_dirty: bool,
    pub color_idx: u8,
    pub is_pinned: bool,
    pub category: u8,
}

impl Note {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            title: [0; MAX_TITLE_LEN],
            title_len: 0,
            body: [0; MAX_BODY_LEN],
            body_len: 0,
            created_at: 0,
            updated_at: 0,
            is_dirty: false,
            color_idx: 0,
            is_pinned: false,
            category: 0,
        }
    }

    pub fn new(id: u32, title: &str, body: &str) -> Self {
        let mut note = Self::empty();
        note.id = id;
        note.set_title(title);
        note.set_body(body);
        note.is_dirty = false;
        note
    }

    pub fn set_color(&mut self, idx: u8) {
        self.color_idx = idx % 8;
        self.is_dirty = true;
    }

    pub fn toggle_pinned(&mut self) {
        self.is_pinned = !self.is_pinned;
        self.is_dirty = true;
    }

    pub fn set_category(&mut self, cat: u8) {
        self.category = cat % 5;
        self.is_dirty = true;
    }

    pub fn set_title(&mut self, title: &str) {
        let bytes = title.as_bytes();
        let len = bytes.len().min(MAX_TITLE_LEN);
        self.title[..len].copy_from_slice(&bytes[..len]);
        for b in &mut self.title[len..] {
            *b = 0;
        }
        self.title_len = len;
        self.is_dirty = true;
    }

    pub fn set_body(&mut self, body: &str) {
        let bytes = body.as_bytes();
        let len = bytes.len().min(MAX_BODY_LEN);
        self.body[..len].copy_from_slice(&bytes[..len]);
        for b in &mut self.body[len..] {
            *b = 0;
        }
        self.body_len = len;
        self.is_dirty = true;
    }

    pub fn as_title_str(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("")
    }

    pub fn as_body_str(&self) -> &str {
        core::str::from_utf8(&self.body[..self.body_len]).unwrap_or("")
    }

    pub fn title_bytes(&self) -> &[u8] {
        &self.title[..self.title_len]
    }

    pub fn body_bytes(&self) -> &[u8] {
        &self.body[..self.body_len]
    }
}

// =============================================================================
// Notes Store
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotesStore {
    pub notes: [Note; MAX_NOTES],
    pub count: usize,
    pub next_id: u32,
}

impl NotesStore {
    pub const fn new() -> Self {
        Self {
            notes: [Note::empty(); MAX_NOTES],
            count: 0,
            next_id: 1,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn create_note(&mut self, title: &str, body: &str) -> Option<u32> {
        if self.count >= MAX_NOTES {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let mut note = Note::new(id, title, body);
        note.is_dirty = true;
        self.notes[self.count] = note;
        self.count += 1;
        Some(id)
    }

    pub fn get_note(&self, id: u32) -> Option<&Note> {
        for i in 0..self.count {
            if self.notes[i].id == id {
                return Some(&self.notes[i]);
            }
        }
        None
    }

    pub fn get_note_mut(&mut self, id: u32) -> Option<&mut Note> {
        for i in 0..self.count {
            if self.notes[i].id == id {
                return Some(&mut self.notes[i]);
            }
        }
        None
    }

    pub fn delete_note(&mut self, id: u32) -> bool {
        let mut found_idx = None;
        for i in 0..self.count {
            if self.notes[i].id == id {
                found_idx = Some(i);
                break;
            }
        }
        if let Some(idx) = found_idx {
            for i in idx..(self.count - 1) {
                self.notes[i] = self.notes[i + 1];
            }
            self.count -= 1;
            self.notes[self.count] = Note::empty();
            true
        } else {
            false
        }
    }

    pub fn get_by_index(&self, index: usize) -> Option<&Note> {
        if index < self.count {
            Some(&self.notes[index])
        } else {
            None
        }
    }

    pub fn get_by_index_mut(&mut self, index: usize) -> Option<&mut Note> {
        if index < self.count {
            Some(&mut self.notes[index])
        } else {
            None
        }
    }

    pub fn seed_defaults(&mut self) {
        self.count = 0;
        self.next_id = 1;
        self.create_note(
            "InterOS Architecture",
            "True zero-ambient authority OS with microkernel security, zero heap allocations, and hardware sovereignty.",
        );
        self.create_note(
            "Launch Checklist",
            "[x] Microkernel scanout pipeline\n[x] Zero heap allocations\n[ ] Google Keep mobile UI\n[ ] Interactive checklists",
        );
        self.create_note(
            "Security Directives",
            "Panic handler resilience: never take blocking locks. Verify all contract suites before release.",
        );
        self.create_note(
            "Hardware Specs",
            "FHD+ 1080x2160 scanout, 3:2 bilinear upscaling from 720x1440, TrueType 4-bit AA font rasterizer.",
        );

        // Assign Google Keep color identities & categories to seed notes
        if self.count >= 4 {
            self.notes[0].color_idx = 2; // Warm Honey / Amber
            self.notes[0].category = 4;  // System
            self.notes[0].is_pinned = true;

            self.notes[1].color_idx = 5; // Dusk Sky / Cerulean
            self.notes[1].category = 1;  // Work
            self.notes[1].is_pinned = true;

            self.notes[2].color_idx = 1; // Coral / Peach
            self.notes[2].category = 4;  // System
            self.notes[2].is_pinned = false;

            self.notes[3].color_idx = 3; // Fresh Sage / Mint
            self.notes[3].category = 3;  // Ideas
            self.notes[3].is_pinned = false;
        }

        for i in 0..self.count {
            self.notes[i].is_dirty = false;
            self.notes[i].created_at = 1729330000;
            self.notes[i].updated_at = 1729330000;
        }
    }

    pub fn serialize(&self, buf: &mut [u8]) -> Result<usize, StoreError> {
        if buf.len() < 12 {
            return Err(StoreError::BufferTooSmall);
        }

        buf[0..6].copy_from_slice(&MAGIC_HEADER);
        buf[6..8].copy_from_slice(&(self.count as u16).to_le_bytes());

        let mut offset = 12;
        for i in 0..self.count {
            let note = &self.notes[i];
            let rec_size = 4 + 2 + note.title_len + 2 + note.body_len + 8 + 8 + 1 + 1 + 1 + 1;
            if offset + rec_size > buf.len() {
                return Err(StoreError::BufferTooSmall);
            }

            // id: u32
            buf[offset..offset + 4].copy_from_slice(&note.id.to_le_bytes());
            offset += 4;

            // title_len: u16 & title
            buf[offset..offset + 2].copy_from_slice(&(note.title_len as u16).to_le_bytes());
            offset += 2;
            buf[offset..offset + note.title_len].copy_from_slice(&note.title[..note.title_len]);
            offset += note.title_len;

            // body_len: u16 & body
            buf[offset..offset + 2].copy_from_slice(&(note.body_len as u16).to_le_bytes());
            offset += 2;
            buf[offset..offset + note.body_len].copy_from_slice(&note.body[..note.body_len]);
            offset += note.body_len;

            // created_at: u64
            buf[offset..offset + 8].copy_from_slice(&note.created_at.to_le_bytes());
            offset += 8;

            // updated_at: u64
            buf[offset..offset + 8].copy_from_slice(&note.updated_at.to_le_bytes());
            offset += 8;

            // is_dirty: u8
            buf[offset] = if note.is_dirty { 1 } else { 0 };
            offset += 1;

            // color_idx: u8
            buf[offset] = note.color_idx;
            offset += 1;

            // is_pinned: u8
            buf[offset] = if note.is_pinned { 1 } else { 0 };
            offset += 1;

            // category: u8
            buf[offset] = note.category;
            offset += 1;
        }

        let checksum = fletcher32(&buf[12..offset]);
        buf[8..12].copy_from_slice(&checksum.to_le_bytes());

        Ok(offset)
    }

    pub fn deserialize(&mut self, buf: &[u8]) -> Result<(), StoreError> {
        if buf.len() < 12 {
            return Err(StoreError::UnexpectedEof);
        }

        if &buf[0..4] != b"NOTE" {
            return Err(StoreError::InvalidMagic);
        }
        let version = buf[4];
        if (version != 1 && version != 2) || buf[5] != 0 {
            return Err(StoreError::UnsupportedVersion);
        }

        let count = u16::from_le_bytes([buf[6], buf[7]]) as usize;
        if count > MAX_NOTES {
            return Err(StoreError::CapacityExceeded);
        }

        let expected_checksum = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);

        let mut temp_notes = [Note::empty(); MAX_NOTES];
        let mut offset = 12;

        for i in 0..count {
            if offset + 4 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let id = u32::from_le_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]);
            offset += 4;

            if offset + 2 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let title_len = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as usize;
            offset += 2;
            if title_len > MAX_TITLE_LEN {
                return Err(StoreError::CorruptedData);
            }
            if offset + title_len > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let mut title = [0u8; MAX_TITLE_LEN];
            title[..title_len].copy_from_slice(&buf[offset..offset + title_len]);
            offset += title_len;

            if offset + 2 > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let body_len = u16::from_le_bytes([buf[offset], buf[offset + 1]]) as usize;
            offset += 2;
            if body_len > MAX_BODY_LEN {
                return Err(StoreError::CorruptedData);
            }
            if offset + body_len > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let mut body = [0u8; MAX_BODY_LEN];
            body[..body_len].copy_from_slice(&buf[offset..offset + body_len]);
            offset += body_len;

            let extra_bytes = if version == 2 { 20 } else { 17 };
            if offset + extra_bytes > buf.len() {
                return Err(StoreError::UnexpectedEof);
            }
            let created_at = u64::from_le_bytes([
                buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
            ]);
            offset += 8;

            let updated_at = u64::from_le_bytes([
                buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
            ]);
            offset += 8;

            let is_dirty = buf[offset] != 0;
            offset += 1;

            let (color_idx, is_pinned, category) = if version == 2 {
                let c = buf[offset];
                let p = buf[offset + 1] != 0;
                let cat = buf[offset + 2];
                offset += 3;
                (c, p, cat)
            } else {
                (0, false, 0)
            };

            temp_notes[i] = Note {
                id,
                title,
                title_len,
                body,
                body_len,
                created_at,
                updated_at,
                is_dirty,
                color_idx,
                is_pinned,
                category,
            };
        }

        let actual_checksum = fletcher32(&buf[12..offset]);
        if actual_checksum != expected_checksum {
            return Err(StoreError::ChecksumMismatch);
        }

        self.notes = temp_notes;
        self.count = count;

        let mut max_id = 0;
        for i in 0..count {
            if self.notes[i].id > max_id {
                max_id = self.notes[i].id;
            }
        }
        self.next_id = max_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        Ok(())
    }
}

// =============================================================================
// Text Editing Buffer Engine
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEditorBuffer {
    pub buf: [u8; MAX_BODY_LEN],
    pub len: usize,
    pub cursor: usize,
}

impl TextEditorBuffer {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_BODY_LEN],
            len: 0,
            cursor: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        let mut buf = Self::new();
        buf.insert_str(s);
        buf
    }

    pub fn load_from_slice(&mut self, slice: &[u8]) {
        let l = slice.len().min(MAX_BODY_LEN);
        self.buf[..l].copy_from_slice(&slice[..l]);
        for b in &mut self.buf[l..] {
            *b = 0;
        }
        self.len = l;
        self.cursor = l;
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.cursor = 0;
        for b in &mut self.buf {
            *b = 0;
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn insert_char(&mut self, c: char) -> bool {
        let mut enc = [0u8; 4];
        let enc_str = c.encode_utf8(&mut enc);
        let char_len = enc_str.len();

        if self.len + char_len > MAX_BODY_LEN {
            return false;
        }

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, self.cursor + char_len);
        }
        self.buf[self.cursor..self.cursor + char_len].copy_from_slice(enc_str.as_bytes());
        self.cursor += char_len;
        self.len += char_len;
        true
    }

    pub fn insert_str(&mut self, s: &str) -> bool {
        let bytes = s.as_bytes();
        let bytes_len = bytes.len();

        if self.len + bytes_len > MAX_BODY_LEN {
            return false;
        }

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, self.cursor + bytes_len);
        }
        self.buf[self.cursor..self.cursor + bytes_len].copy_from_slice(bytes);
        self.cursor += bytes_len;
        self.len += bytes_len;
        true
    }

    pub fn toggle_checklist_at(&mut self, line_start: usize) -> bool {
        if line_start + 3 <= self.len {
            if self.buf[line_start] == b'[' && self.buf[line_start + 2] == b']' {
                if self.buf[line_start + 1] == b' ' {
                    self.buf[line_start + 1] = b'x';
                    return true;
                } else if self.buf[line_start + 1] == b'x' || self.buf[line_start + 1] == b'X' {
                    self.buf[line_start + 1] = b' ';
                    return true;
                }
            }
        }
        false
    }

    pub fn insert_checklist_item(&mut self) -> bool {
        if self.len == 0 {
            self.insert_str("[ ] ")
        } else {
            self.insert_str("\n[ ] ")
        }
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }

        let mut prev = self.cursor - 1;
        while prev > 0 && (self.buf[prev] & 0xC0) == 0x80 {
            prev -= 1;
        }
        let char_len = self.cursor - prev;

        if self.cursor < self.len {
            self.buf.copy_within(self.cursor..self.len, prev);
        }
        self.cursor = prev;
        self.len -= char_len;
        self.buf[self.len..self.len + char_len].fill(0);
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.len {
            return false;
        }

        let mut next = self.cursor + 1;
        while next < self.len && (self.buf[next] & 0xC0) == 0x80 {
            next += 1;
        }
        let char_len = next - self.cursor;

        if next < self.len {
            self.buf.copy_within(next..self.len, self.cursor);
        }
        self.len -= char_len;
        self.buf[self.len..self.len + char_len].fill(0);
        true
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut prev = self.cursor - 1;
        while prev > 0 && (self.buf[prev] & 0xC0) == 0x80 {
            prev -= 1;
        }
        self.cursor = prev;
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor >= self.len {
            return;
        }
        let mut next = self.cursor + 1;
        while next < self.len && (self.buf[next] & 0xC0) == 0x80 {
            next += 1;
        }
        self.cursor = next;
    }

    pub fn move_cursor_home(&mut self) {
        let mut pos = self.cursor;
        while pos > 0 && self.buf[pos - 1] != b'\n' {
            pos -= 1;
        }
        self.cursor = pos;
    }

    pub fn move_cursor_end(&mut self) {
        let mut pos = self.cursor;
        while pos < self.len && self.buf[pos] != b'\n' {
            pos += 1;
        }
        self.cursor = pos;
    }

    pub fn move_cursor_to_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_cursor_to_end(&mut self) {
        self.cursor = self.len;
    }

    pub fn compute_lines(&self, max_chars_per_line: usize, out: &mut [LineSpan]) -> usize {
        if out.is_empty() {
            return 0;
        }
        if self.len == 0 {
            out[0] = LineSpan { start: 0, end: 0 };
            return 1;
        }
        let max_chars = max_chars_per_line.max(1);
        let mut line_count = 0;
        let mut line_start = 0;

        while line_start < self.len && line_count < out.len() {
            let mut char_count = 0;
            let mut last_space = None;
            let mut idx = line_start;
            let mut line_end = line_start;

            while idx < self.len {
                let b = self.buf[idx];
                if b == b'\n' {
                    line_end = idx;
                    idx += 1;
                    break;
                }
                if b == b' ' {
                    last_space = Some(idx);
                }
                char_count += 1;
                if char_count > max_chars {
                    if let Some(space_idx) = last_space {
                        line_end = space_idx;
                        idx = space_idx + 1;
                    } else {
                        line_end = idx;
                    }
                    break;
                }
                idx += 1;
                line_end = idx;
            }

            out[line_count] = LineSpan {
                start: line_start,
                end: line_end,
            };
            line_count += 1;
            line_start = idx;
        }

        if self.len > 0 && self.buf[self.len - 1] == b'\n' && line_count < out.len() {
            out[line_count] = LineSpan {
                start: self.len,
                end: self.len,
            };
            line_count += 1;
        }

        line_count
    }

    pub fn line_count(&self, max_chars_per_line: usize) -> usize {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        self.compute_lines(max_chars_per_line, &mut spans)
    }

    pub fn cursor_line_col(&self, max_chars_per_line: usize) -> (usize, usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(max_chars_per_line, &mut spans);
        if total == 0 {
            return (0, 0);
        }
        for (i, span) in spans[..total].iter().enumerate() {
            if self.cursor >= span.start && (self.cursor <= span.end || i == total - 1) {
                let col = self.cursor.saturating_sub(span.start);
                return (i, col);
            }
        }
        (total.saturating_sub(1), 0)
    }

    pub fn move_cursor_up(&mut self, max_chars_per_line: usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(max_chars_per_line, &mut spans);
        if total == 0 {
            return;
        }
        let (line, col) = self.cursor_line_col(max_chars_per_line);
        if line > 0 {
            let target_span = spans[line - 1];
            self.cursor = (target_span.start + col).min(target_span.end);
        }
    }

    pub fn move_cursor_down(&mut self, max_chars_per_line: usize) {
        let mut spans = [LineSpan { start: 0, end: 0 }; 64];
        let total = self.compute_lines(max_chars_per_line, &mut spans);
        if total == 0 {
            return;
        }
        let (line, col) = self.cursor_line_col(max_chars_per_line);
        if line + 1 < total {
            let target_span = spans[line + 1];
            self.cursor = (target_span.start + col).min(target_span.end);
        }
    }
}

// =============================================================================
// Fletcher-32 Checksum & Serialization Errors
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    BufferTooSmall,
    InvalidMagic,
    UnsupportedVersion,
    ChecksumMismatch,
    CorruptedData,
    CapacityExceeded,
    UnexpectedEof,
}

pub fn fletcher32(data: &[u8]) -> u32 {
    let mut sum1: u32 = 0;
    let mut sum2: u32 = 0;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let word = if i + 1 < len {
            u16::from_le_bytes([data[i], data[i + 1]]) as u32
        } else {
            data[i] as u32
        };
        sum1 = (sum1 + word) % 65535;
        sum2 = (sum2 + sum1) % 65535;
        i += 2;
    }

    (sum2 << 16) | sum1
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_creation_and_fields() {
        let mut note = Note::new(42, "Architecture", "Microkernel security");
        assert_eq!(note.id, 42);
        assert_eq!(note.as_title_str(), "Architecture");
        assert_eq!(note.as_body_str(), "Microkernel security");
        assert_eq!(note.title_bytes(), b"Architecture");
        assert_eq!(note.body_bytes(), b"Microkernel security");
        assert!(!note.is_dirty);

        note.set_title("New Title");
        assert_eq!(note.as_title_str(), "New Title");
        assert!(note.is_dirty);

        note.set_body("Updated body text");
        assert_eq!(note.as_body_str(), "Updated body text");
        assert!(note.is_dirty);
    }

    #[test]
    fn test_notes_store_operations_and_ordering() {
        let mut store = NotesStore::new();
        assert_eq!(store.count(), 0);

        let id1 = store.create_note("First", "Body 1").expect("id1");
        let id2 = store.create_note("Second", "Body 2").expect("id2");
        let id3 = store.create_note("Third", "Body 3").expect("id3");

        assert_eq!(store.count(), 3);
        assert_eq!(store.get_note(id1).unwrap().as_title_str(), "First");
        assert_eq!(store.get_note(id2).unwrap().as_title_str(), "Second");
        assert_eq!(store.get_note(id3).unwrap().as_title_str(), "Third");

        // Index ordering
        assert_eq!(store.get_by_index(0).unwrap().id, id1);
        assert_eq!(store.get_by_index(1).unwrap().id, id2);
        assert_eq!(store.get_by_index(2).unwrap().id, id3);

        // Delete middle note and verify compact ordering
        let deleted = store.delete_note(id2);
        assert!(deleted);
        assert_eq!(store.count(), 2);
        assert_eq!(store.get_by_index(0).unwrap().id, id1);
        assert_eq!(store.get_by_index(1).unwrap().id, id3);
        assert!(store.get_note(id2).is_none());

        // Modify note via get_note_mut
        if let Some(n) = store.get_note_mut(id3) {
            n.set_title("Third Modified");
        }
        assert_eq!(store.get_note(id3).unwrap().as_title_str(), "Third Modified");
    }

    #[test]
    fn test_text_editor_operations() {
        let mut ed = TextEditorBuffer::new();
        assert_eq!(ed.len, 0);
        assert_eq!(ed.cursor, 0);

        // Insert characters
        assert!(ed.insert_char('H'));
        assert!(ed.insert_char('e'));
        assert!(ed.insert_char('l'));
        assert!(ed.insert_char('l'));
        assert!(ed.insert_char('o'));
        assert_eq!(ed.as_str(), "Hello");
        assert_eq!(ed.cursor, 5);

        // Cursor movement
        ed.move_cursor_left();
        assert_eq!(ed.cursor, 4);
        ed.move_cursor_left();
        assert_eq!(ed.cursor, 3);
        ed.move_cursor_right();
        assert_eq!(ed.cursor, 4);

        // Backspace inside text
        assert!(ed.backspace());
        assert_eq!(ed.as_str(), "Helo");
        assert_eq!(ed.cursor, 3);

        // Delete
        assert!(ed.delete());
        assert_eq!(ed.as_str(), "Hel");
        assert_eq!(ed.cursor, 3);

        // Home and End
        ed.move_cursor_home();
        assert_eq!(ed.cursor, 0);
        ed.move_cursor_end();
        assert_eq!(ed.cursor, 3);

        // Multiline insertion and navigation
        ed.insert_char('\n');
        ed.insert_str("World");
        assert_eq!(ed.as_str(), "Hel\nWorld");
        assert_eq!(ed.cursor, 9);

        ed.move_cursor_home();
        assert_eq!(ed.cursor, 4); // Start of "World" line
        ed.move_cursor_end();
        assert_eq!(ed.cursor, 9);
    }

    #[test]
    fn test_text_editor_line_wrapping() {
        let mut ed = TextEditorBuffer::new();
        ed.insert_str("The quick brown fox jumps over the lazy dog");

        let mut spans = [LineSpan { start: 0, end: 0 }; 16];
        let count = ed.compute_lines(16, &mut spans);

        assert!(count >= 3);
        // First line: "The quick brown" (0..15)
        assert_eq!(&ed.buf[spans[0].start..spans[0].end], b"The quick brown");
        // Second line: "fox jumps over" (16..30)
        assert_eq!(&ed.buf[spans[1].start..spans[1].end], b"fox jumps over");
        // Third line: "the lazy dog" (31..43)
        assert_eq!(&ed.buf[spans[2].start..spans[2].end], b"the lazy dog");

        // Cursor line and col check
        ed.cursor = 0;
        assert_eq!(ed.cursor_line_col(16), (0, 0));
        ed.cursor = 16;
        assert_eq!(ed.cursor_line_col(16), (1, 0));
    }

    #[test]
    fn test_binary_serialization_roundtrip() {
        let mut store = NotesStore::new();
        store.seed_defaults();
        assert_eq!(store.count(), 4);

        let mut buf = [0u8; 4096];
        let bytes_written = store.serialize(&mut buf).expect("serialize should succeed");
        assert!(bytes_written > 12);

        // Verify header
        assert_eq!(&buf[0..6], &MAGIC_HEADER);
        let count = u16::from_le_bytes([buf[6], buf[7]]);
        assert_eq!(count, 4);

        // Deserialize into new store
        let mut restored = NotesStore::new();
        restored.deserialize(&buf[..bytes_written]).expect("deserialize should succeed");

        assert_eq!(restored.count(), 4);
        for i in 0..4 {
            let orig = store.get_by_index(i).unwrap();
            let rest = restored.get_by_index(i).unwrap();
            assert_eq!(orig.id, rest.id);
            assert_eq!(orig.as_title_str(), rest.as_title_str());
            assert_eq!(orig.as_body_str(), rest.as_body_str());
            assert_eq!(orig.created_at, rest.created_at);
            assert_eq!(orig.updated_at, rest.updated_at);
            assert_eq!(orig.color_idx, rest.color_idx);
            assert_eq!(orig.is_pinned, rest.is_pinned);
            assert_eq!(orig.category, rest.category);
        }
    }

    #[test]
    fn test_corrupted_data_rejection() {
        let mut store = NotesStore::new();
        store.create_note("Valid", "Data payload");

        let mut buf = [0u8; 2048];
        let bytes_written = store.serialize(&mut buf).expect("serialize");

        // 1. Bit flip in payload triggers ChecksumMismatch
        let mut corrupted_payload = buf;
        corrupted_payload[20] ^= 0xFF;
        let mut target = NotesStore::new();
        let res = target.deserialize(&corrupted_payload[..bytes_written]);
        assert_eq!(res, Err(StoreError::ChecksumMismatch));
        assert_eq!(target.count(), 0); // Unaltered

        // 2. Corrupt magic triggers InvalidMagic
        let mut corrupted_magic = buf;
        corrupted_magic[0] = b'X';
        let res = target.deserialize(&corrupted_magic[..bytes_written]);
        assert_eq!(res, Err(StoreError::InvalidMagic));

        // 3. Truncated buffer triggers UnexpectedEof
        let res = target.deserialize(&buf[..10]);
        assert_eq!(res, Err(StoreError::UnexpectedEof));

        // 4. Unsupported version
        let mut bad_ver = buf;
        bad_ver[4] = 99; // unsupported version
        let res = target.deserialize(&bad_ver[..bytes_written]);
        assert_eq!(res, Err(StoreError::UnsupportedVersion));
    }

    #[test]
    fn test_store_capacity_limits() {
        let mut store = NotesStore::new();
        for i in 0..MAX_NOTES {
            let res = store.create_note("Test Note", "Body content");
            assert!(res.is_some(), "Should allow creating note {}", i);
        }
        assert_eq!(store.count(), MAX_NOTES);

        // 17th note should fail
        let extra = store.create_note("Overflow", "Cannot fit");
        assert!(extra.is_none());
        assert_eq!(store.count(), MAX_NOTES);

        // Capacity exceeded on deserialization
        let mut fake_buf = [0u8; 32];
        fake_buf[0..6].copy_from_slice(&MAGIC_HEADER);
        fake_buf[6..8].copy_from_slice(&((MAX_NOTES as u16 + 1).to_le_bytes()));
        let mut target = NotesStore::new();
        assert_eq!(target.deserialize(&fake_buf), Err(StoreError::CapacityExceeded));
    }

    #[test]
    fn test_text_editor_up_down_and_bounds() {
        let mut ed = TextEditorBuffer::new();
        ed.insert_str("Line 1\nLine 2 is longer\nLine 3");

        // Move to start of line 2
        ed.cursor = 7;
        assert_eq!(ed.cursor_line_col(36), (1, 0));

        // Move up to line 1
        ed.move_cursor_up(36);
        assert_eq!(ed.cursor_line_col(36), (0, 0));
        assert_eq!(ed.cursor, 0);

        // Move down to line 2
        ed.move_cursor_down(36);
        assert_eq!(ed.cursor_line_col(36), (1, 0));
        assert_eq!(ed.cursor, 7);

        // Move down to line 3
        ed.move_cursor_down(36);
        assert_eq!(ed.cursor_line_col(36), (2, 0));
        assert_eq!(ed.cursor, 24);

        // Cannot move down past end
        ed.move_cursor_down(36);
        assert_eq!(ed.cursor_line_col(36), (2, 0));

        // Start / End
        ed.move_cursor_to_start();
        assert_eq!(ed.cursor, 0);
        ed.move_cursor_to_end();
        assert_eq!(ed.cursor, ed.len);
    }

    #[test]
    fn test_checklist_support() {
        let mut ed = TextEditorBuffer::new();
        ed.insert_checklist_item();
        ed.insert_str("Buy milk");
        assert_eq!(ed.as_str(), "[ ] Buy milk");

        // Toggle to checked
        assert!(ed.toggle_checklist_at(0));
        assert_eq!(ed.as_str(), "[x] Buy milk");

        // Toggle back to unchecked
        assert!(ed.toggle_checklist_at(0));
        assert_eq!(ed.as_str(), "[ ] Buy milk");
    }
}
