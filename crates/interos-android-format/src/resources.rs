// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android `resources.arsc` Binary Resource Table Parser & Resolver
//!
//! Parses compiled Android resource tables (`resources.arsc`) to resolve resource
//! identifiers (0xPPTTEEEE) to strings, colors, dimensions, booleans, and references
//! without string fabrication or ambient authority.
//!
//! Complies with AOSP ResourceTypes.h specification:
//! - RES_TABLE_TYPE (0x0002)
//! - RES_STRING_POOL_TYPE (0x0001)
//! - RES_TABLE_PACKAGE_TYPE (0x0200)
//! - RES_TABLE_TYPE_TYPE (0x0201)
//! - RES_TABLE_TYPE_SPEC_TYPE (0x0202)
//!
//! Resource ID format: 0xPPTTEEEE
//! - PP: Package ID (0x7f for application, 0x01 for system framework)
//! - TT: Type ID (1-based index matching type strings: 1=attr, 2=drawable, 3=layout, 4=string, ...)
//! - EEEE: Entry Index (0-based)

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const RES_NULL_TYPE: u16 = 0x0000;
pub const RES_STRING_POOL_TYPE: u16 = 0x0001;
pub const RES_TABLE_TYPE: u16 = 0x0002;
pub const RES_XML_TYPE: u16 = 0x0003;
pub const RES_TABLE_PACKAGE_TYPE: u16 = 0x0200;
pub const RES_TABLE_TYPE_TYPE: u16 = 0x0201;
pub const RES_TABLE_TYPE_SPEC_TYPE: u16 = 0x0202;
pub const RES_TABLE_LIBRARY_TYPE: u16 = 0x0203;

pub const TYPE_NULL: u8 = 0;
pub const TYPE_REFERENCE: u8 = 1;
pub const TYPE_ATTRIBUTE: u8 = 2;
pub const TYPE_STRING: u8 = 3;
pub const TYPE_FLOAT: u8 = 4;
pub const TYPE_DIMENSION: u8 = 5;
pub const TYPE_FRACTION: u8 = 6;
pub const TYPE_INT_DEC: u8 = 16;
pub const TYPE_INT_HEX: u8 = 17;
pub const TYPE_INT_BOOLEAN: u8 = 18;
pub const TYPE_INT_COLOR_ARGB8: u8 = 28;
pub const TYPE_INT_COLOR_RGB8: u8 = 29;
pub const TYPE_INT_COLOR_ARGB4: u8 = 30;
pub const TYPE_INT_COLOR_RGB4: u8 = 31;

pub const NO_ENTRY: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    TooSmall,
    InvalidHeader,
    UnsupportedVersion,
    MalformedChunk,
    MalformedStringPool,
    PackageNotFound,
    ResourceNotFound(u32),
}

/// Resolved resource value variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceValue {
    String(String),
    Color(u32),
    Integer(i32),
    Boolean(bool),
    Dimension(u32),
    Reference(u32),
    Null,
}

/// Parsed Android String Pool chunk.
#[derive(Debug, Clone)]
pub struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    pub fn parse(data: &[u8], offset: usize) -> Result<(Self, usize), ResourceError> {
        if offset + 28 > data.len() {
            return Err(ResourceError::TooSmall);
        }

        let chunk_type = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
        if chunk_type != RES_STRING_POOL_TYPE {
            return Err(ResourceError::MalformedChunk);
        }

        let chunk_size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if offset + chunk_size > data.len() || chunk_size < 28 {
            return Err(ResourceError::MalformedChunk);
        }

        let string_count = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap());
        let is_utf8 = (flags & (1 << 8)) != 0;
        let strings_start = u32::from_le_bytes(data[offset + 20..offset + 24].try_into().unwrap()) as usize;

        if offset + strings_start > data.len() {
            return Err(ResourceError::MalformedStringPool);
        }

        let offsets_start = offset + 28;
        if offsets_start + string_count * 4 > offset + strings_start {
            return Err(ResourceError::MalformedStringPool);
        }

        let pool_data_start = offset + strings_start;
        let pool_data_end = offset + chunk_size;
        let mut strings = Vec::with_capacity(string_count);

        for i in 0..string_count {
            let off_pos = offsets_start + i * 4;
            let str_rel_off = u32::from_le_bytes(data[off_pos..off_pos + 4].try_into().unwrap()) as usize;
            let abs_str_off = pool_data_start + str_rel_off;

            if abs_str_off >= pool_data_end {
                strings.push(String::new());
                continue;
            }

            if is_utf8 {
                // UTF-8 string format: [char_len][byte_len][chars][null]
                let mut cur = abs_str_off;
                // Skip character length (1 or 2 bytes)
                if cur < pool_data_end && (data[cur] & 0x80) != 0 {
                    cur += 2;
                } else {
                    cur += 1;
                }
                // Byte length (1 or 2 bytes)
                let byte_len = if cur < pool_data_end && (data[cur] & 0x80) != 0 {
                    let len = (((data[cur] & 0x7F) as usize) << 8) | (data[cur + 1] as usize);
                    cur += 2;
                    len
                } else if cur < pool_data_end {
                    let len = data[cur] as usize;
                    cur += 1;
                    len
                } else {
                    0
                };

                let end = cur + byte_len;
                if end <= pool_data_end {
                    let s = core::str::from_utf8(&data[cur..end]).unwrap_or("");
                    strings.push(String::from(s));
                } else {
                    strings.push(String::new());
                }
            } else {
                // UTF-16 string format: [char_len: 2 bytes][chars: char_len * 2 bytes][null: 2 bytes]
                let mut cur = abs_str_off;
                if cur + 2 <= pool_data_end {
                    let len_code = u16::from_le_bytes([data[cur], data[cur + 1]]) as usize;
                    cur += 2;
                    let u16_len = if (len_code & 0x8000) != 0 && cur + 2 <= pool_data_end {
                        let high = (len_code & 0x7FFF) << 16;
                        let low = u16::from_le_bytes([data[cur], data[cur + 1]]) as usize;
                        cur += 2;
                        high | low
                    } else {
                        len_code
                    };

                    let byte_len = u16_len * 2;
                    if cur + byte_len <= pool_data_end {
                        let mut u16_chars = Vec::with_capacity(u16_len);
                        for c_idx in 0..u16_len {
                            let ch = u16::from_le_bytes([data[cur + c_idx * 2], data[cur + c_idx * 2 + 1]]);
                            u16_chars.push(ch);
                        }
                        let s = String::from_utf16_lossy(&u16_chars);
                        strings.push(s);
                    } else {
                        strings.push(String::new());
                    }
                } else {
                    strings.push(String::new());
                }
            }
        }

        Ok((Self { strings }, offset + chunk_size))
    }

    pub fn get(&self, idx: usize) -> Option<&str> {
        self.strings.get(idx).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }
}

/// A parsed resource entry within a package type.
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub key_name: String,
    pub value: ResourceValue,
}

/// A resource package containing type specifications and entry tables.
#[derive(Debug, Clone)]
pub struct ResourcePackage {
    pub id: u32,
    pub name: String,
    pub type_strings: Vec<String>,
    pub key_strings: Vec<String>,
    /// Map of (type_id, entry_id) -> ResourceEntry
    pub entries: BTreeMap<(u8, u16), ResourceEntry>,
}

impl ResourcePackage {
    pub fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            type_strings: Vec::new(),
            key_strings: Vec::new(),
            entries: BTreeMap::new(),
        }
    }
}

/// Complete in-memory Android `resources.arsc` table.
#[derive(Debug, Clone)]
pub struct ResourceTable {
    pub global_strings: StringPool,
    pub packages: BTreeMap<u32, ResourcePackage>,
}

impl ResourceTable {
    /// Parses a raw `resources.arsc` file buffer.
    pub fn parse(data: &[u8]) -> Result<Self, ResourceError> {
        if data.len() < 12 {
            return Err(ResourceError::TooSmall);
        }

        let chunk_type = u16::from_le_bytes(data[0..2].try_into().unwrap());
        if chunk_type != RES_TABLE_TYPE {
            return Err(ResourceError::InvalidHeader);
        }

        let header_size = u16::from_le_bytes(data[2..4].try_into().unwrap()) as usize;
        let chunk_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let _package_count = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;

        if chunk_size > data.len() || header_size < 12 {
            return Err(ResourceError::MalformedChunk);
        }

        // Global String Pool
        let (global_strings, mut cursor) = StringPool::parse(data, header_size)?;
        let mut packages = BTreeMap::new();

        while cursor < chunk_size {
            if cursor + 8 > chunk_size {
                break;
            }
            let sub_type = u16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
            let sub_size = u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;

            if sub_size == 0 || cursor + sub_size > chunk_size {
                return Err(ResourceError::MalformedChunk);
            }

            if sub_type == RES_TABLE_PACKAGE_TYPE {
                let pkg = parse_package_chunk(data, cursor, sub_size, &global_strings)?;
                packages.insert(pkg.id, pkg);
            }

            cursor += sub_size;
        }

        Ok(Self {
            global_strings,
            packages,
        })
    }

    /// Resolves a resource identifier (0xPPTTEEEE) to its String representation.
    pub fn get_string(&self, res_id: u32) -> Option<&str> {
        let pkg_id = (res_id >> 24) & 0xFF;
        let type_id = ((res_id >> 16) & 0xFF) as u8;
        let entry_id = (res_id & 0xFFFF) as u16;

        let pkg = self.packages.get(&pkg_id)?;
        let entry = pkg.entries.get(&(type_id, entry_id))?;
        match &entry.value {
            ResourceValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Resolves a resource identifier to its ARGB color value.
    pub fn get_color(&self, res_id: u32) -> Option<u32> {
        let pkg_id = (res_id >> 24) & 0xFF;
        let type_id = ((res_id >> 16) & 0xFF) as u8;
        let entry_id = (res_id & 0xFFFF) as u16;

        let pkg = self.packages.get(&pkg_id)?;
        let entry = pkg.entries.get(&(type_id, entry_id))?;
        match entry.value {
            ResourceValue::Color(c) => Some(c),
            _ => None,
        }
    }

    /// Resolves a resource identifier to its generic ResourceValue.
    pub fn get_value(&self, res_id: u32) -> Option<&ResourceValue> {
        let pkg_id = (res_id >> 24) & 0xFF;
        let type_id = ((res_id >> 16) & 0xFF) as u8;
        let entry_id = (res_id & 0xFFFF) as u16;

        let pkg = self.packages.get(&pkg_id)?;
        let entry = pkg.entries.get(&(type_id, entry_id))?;
        Some(&entry.value)
    }

    /// Resolves the type name and key entry name for a resource ID (e.g. ("string", "app_name")).
    pub fn resolve_entry_name(&self, res_id: u32) -> Option<(&str, &str)> {
        let pkg_id = (res_id >> 24) & 0xFF;
        let type_id = ((res_id >> 16) & 0xFF) as u8;
        let entry_id = (res_id & 0xFFFF) as u16;

        let pkg = self.packages.get(&pkg_id)?;
        let type_idx = (type_id as usize).checked_sub(1)?;
        let type_name = pkg.type_strings.get(type_idx)?;
        let entry = pkg.entries.get(&(type_id, entry_id))?;
        Some((type_name.as_str(), entry.key_name.as_str()))
    }
}

fn parse_package_chunk(
    data: &[u8],
    offset: usize,
    chunk_size: usize,
    global_strings: &StringPool,
) -> Result<ResourcePackage, ResourceError> {
    if chunk_size < 288 {
        return Err(ResourceError::MalformedChunk);
    }

    let pkg_id = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());

    // 128 UTF-16 character package name (256 bytes)
    let mut u16_chars = Vec::with_capacity(128);
    for i in 0..128 {
        let pos = offset + 12 + i * 2;
        let ch = u16::from_le_bytes([data[pos], data[pos + 1]]);
        if ch == 0 {
            break;
        }
        u16_chars.push(ch);
    }
    let pkg_name = String::from_utf16_lossy(&u16_chars);

    let type_strings_off = u32::from_le_bytes(data[offset + 268..offset + 272].try_into().unwrap()) as usize;
    let key_strings_off = u32::from_le_bytes(data[offset + 276..offset + 280].try_into().unwrap()) as usize;

    let mut pkg = ResourcePackage::new(pkg_id, pkg_name);

    // Type strings pool
    if type_strings_off > 0 && offset + type_strings_off < offset + chunk_size {
        let (pool, _) = StringPool::parse(data, offset + type_strings_off)?;
        pkg.type_strings = pool.strings;
    }

    // Key strings pool
    if key_strings_off > 0 && offset + key_strings_off < offset + chunk_size {
        let (pool, _) = StringPool::parse(data, offset + key_strings_off)?;
        pkg.key_strings = pool.strings;
    }

    // Parse TYPE and TYPE_SPEC chunks
    let mut cur = offset + 288;
    let end = offset + chunk_size;

    while cur < end {
        if cur + 8 > end {
            break;
        }
        let sub_type = u16::from_le_bytes(data[cur..cur + 2].try_into().unwrap());
        let sub_size = u32::from_le_bytes(data[cur + 4..cur + 8].try_into().unwrap()) as usize;

        if sub_size == 0 || cur + sub_size > end {
            break;
        }

        if sub_type == RES_TABLE_TYPE_TYPE {
            parse_type_chunk(data, cur, sub_size, &mut pkg, global_strings)?;
        }

        cur += sub_size;
    }

    Ok(pkg)
}

fn parse_type_chunk(
    data: &[u8],
    offset: usize,
    chunk_size: usize,
    pkg: &mut ResourcePackage,
    global_strings: &StringPool,
) -> Result<(), ResourceError> {
    if chunk_size < 52 {
        return Err(ResourceError::MalformedChunk);
    }

    let type_id = data[offset + 8];
    let entry_count = u32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap()) as usize;
    let entries_start = u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap()) as usize;
    let header_size = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;

    let offsets_start = offset + header_size;
    if offsets_start + entry_count * 4 > offset + entries_start || offset + entries_start > offset + chunk_size {
        return Err(ResourceError::MalformedChunk);
    }

    let data_base = offset + entries_start;
    let chunk_end = offset + chunk_size;

    for entry_idx in 0..entry_count {
        let off_pos = offsets_start + entry_idx * 4;
        let entry_off = u32::from_le_bytes(data[off_pos..off_pos + 4].try_into().unwrap());
        if entry_off == NO_ENTRY {
            continue;
        }

        let abs_entry = data_base + (entry_off as usize);
        if abs_entry + 8 > chunk_end {
            continue;
        }

        let entry_size = u16::from_le_bytes(data[abs_entry..abs_entry + 2].try_into().unwrap()) as usize;
        let flags = u16::from_le_bytes(data[abs_entry + 2..abs_entry + 4].try_into().unwrap());
        let key_idx = u32::from_le_bytes(data[abs_entry + 4..abs_entry + 8].try_into().unwrap()) as usize;

        let key_name = pkg
            .key_strings
            .get(key_idx)
            .cloned()
            .unwrap_or_else(|| format!("res_0x{:04x}", entry_idx));

        let is_complex = (flags & 0x0001) != 0;
        if !is_complex {
            let val_pos = abs_entry + entry_size;
            if val_pos + 8 <= chunk_end {
                let data_type = data[val_pos + 3];
                let raw_data = u32::from_le_bytes(data[val_pos + 4..val_pos + 8].try_into().unwrap());

                let value = match data_type {
                    TYPE_STRING => {
                        let s = global_strings.get(raw_data as usize).unwrap_or("");
                        ResourceValue::String(String::from(s))
                    }
                    TYPE_INT_COLOR_ARGB8 | TYPE_INT_COLOR_RGB8 | TYPE_INT_COLOR_ARGB4 | TYPE_INT_COLOR_RGB4 => {
                        ResourceValue::Color(raw_data)
                    }
                    TYPE_INT_DEC | TYPE_INT_HEX => ResourceValue::Integer(raw_data as i32),
                    TYPE_INT_BOOLEAN => ResourceValue::Boolean(raw_data != 0),
                    TYPE_DIMENSION => ResourceValue::Dimension(raw_data),
                    TYPE_REFERENCE => ResourceValue::Reference(raw_data),
                    _ => ResourceValue::Null,
                };

                pkg.entries.insert((type_id, entry_idx as u16), ResourceEntry { key_name, value });
            }
        }
    }

    Ok(())
}

/// Builds a deterministic binary `resources.arsc` table for testing and fixtures.
pub fn create_test_resources_arsc(
    package_name: &str,
    strings: &[(&str, &str)], // (key_name, string_value)
    colors: &[(&str, u32)],    // (key_name, color_argb)
) -> Vec<u8> {
    let mut arsc = Vec::new();

    // 1. Build Global String Pool: contains all string resource values
    let mut val_strings = Vec::new();
    for (_, val) in strings {
        val_strings.push(String::from(*val));
    }

    let mut str_pool = Vec::new();
    let str_pool_hdr_size = 28u16;
    let str_count = val_strings.len() as u32;

    str_pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    str_pool.extend_from_slice(&str_pool_hdr_size.to_le_bytes());
    str_pool.extend_from_slice(&0u32.to_le_bytes()); // placeholder chunk size
    str_pool.extend_from_slice(&str_count.to_le_bytes());
    str_pool.extend_from_slice(&0u32.to_le_bytes()); // style count = 0
    str_pool.extend_from_slice(&(1u32 << 8).to_le_bytes()); // UTF-8 flag

    let offsets_table_len = val_strings.len() * 4;
    let strings_data_offset = 28 + offsets_table_len;
    str_pool.extend_from_slice(&(strings_data_offset as u32).to_le_bytes());
    str_pool.extend_from_slice(&0u32.to_le_bytes()); // styles start

    let mut str_data = Vec::new();
    for s in &val_strings {
        let off = str_data.len() as u32;
        str_pool.extend_from_slice(&off.to_le_bytes());
        let bytes = s.as_bytes();
        str_data.push(bytes.len() as u8);
        str_data.push(bytes.len() as u8);
        str_data.extend_from_slice(bytes);
        str_data.push(0);
    }
    str_pool.extend_from_slice(&str_data);
    while str_pool.len() % 4 != 0 {
        str_pool.push(0);
    }
    let pool_size = str_pool.len() as u32;
    str_pool[4..8].copy_from_slice(&pool_size.to_le_bytes());

    // 2. Build Package Chunk
    let mut pkg_chunk = Vec::new();
    let pkg_id = 0x7fu32; // Standard application package ID
    pkg_chunk.extend_from_slice(&RES_TABLE_PACKAGE_TYPE.to_le_bytes());
    pkg_chunk.extend_from_slice(&288u16.to_le_bytes()); // header size = 288
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes());   // placeholder chunk size
    pkg_chunk.extend_from_slice(&pkg_id.to_le_bytes());

    // 128 UTF-16 characters for package name
    let mut name_u16 = Vec::new();
    for c in package_name.encode_utf16() {
        name_u16.push(c);
    }
    while name_u16.len() < 128 {
        name_u16.push(0);
    }
    for c in &name_u16 {
        pkg_chunk.extend_from_slice(&c.to_le_bytes());
    }

    // Placeholder offsets for typeStrings and keyStrings
    let type_strings_off_pos = pkg_chunk.len();
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // typeStrings offset
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // lastPublicType
    let key_strings_off_pos = pkg_chunk.len();
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // keyStrings offset
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // lastPublicKey
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // typeIdOffset (pad header to 288 bytes)

    // Types pool: ["attr", "drawable", "layout", "string", "color"]
    let types = ["attr", "drawable", "layout", "string", "color"];
    let type_strings_start = pkg_chunk.len() as u32;
    pkg_chunk[type_strings_off_pos..type_strings_off_pos + 4].copy_from_slice(&type_strings_start.to_le_bytes());

    let mut type_pool = Vec::new();
    type_pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    type_pool.extend_from_slice(&28u16.to_le_bytes());
    type_pool.extend_from_slice(&0u32.to_le_bytes()); // placeholder
    type_pool.extend_from_slice(&(types.len() as u32).to_le_bytes());
    type_pool.extend_from_slice(&0u32.to_le_bytes());
    type_pool.extend_from_slice(&(1u32 << 8).to_le_bytes());
    let type_data_off = 28 + types.len() * 4;
    type_pool.extend_from_slice(&(type_data_off as u32).to_le_bytes());
    type_pool.extend_from_slice(&0u32.to_le_bytes());

    let mut type_data = Vec::new();
    for t in &types {
        let off = type_data.len() as u32;
        type_pool.extend_from_slice(&off.to_le_bytes());
        let b = t.as_bytes();
        type_data.push(b.len() as u8);
        type_data.push(b.len() as u8);
        type_data.extend_from_slice(b);
        type_data.push(0);
    }
    type_pool.extend_from_slice(&type_data);
    while type_pool.len() % 4 != 0 {
        type_pool.push(0);
    }
    let type_pool_sz = type_pool.len() as u32;
    type_pool[4..8].copy_from_slice(&type_pool_sz.to_le_bytes());
    pkg_chunk.extend_from_slice(&type_pool);

    // Key strings pool: all keys from strings and colors
    let key_strings_start = pkg_chunk.len() as u32;
    pkg_chunk[key_strings_off_pos..key_strings_off_pos + 4].copy_from_slice(&key_strings_start.to_le_bytes());

    let mut keys = Vec::new();
    for (k, _) in strings {
        keys.push(*k);
    }
    for (k, _) in colors {
        keys.push(*k);
    }

    let mut key_pool = Vec::new();
    key_pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    key_pool.extend_from_slice(&28u16.to_le_bytes());
    key_pool.extend_from_slice(&0u32.to_le_bytes());
    key_pool.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    key_pool.extend_from_slice(&0u32.to_le_bytes());
    key_pool.extend_from_slice(&(1u32 << 8).to_le_bytes());
    let key_data_off = 28 + keys.len() * 4;
    key_pool.extend_from_slice(&(key_data_off as u32).to_le_bytes());
    key_pool.extend_from_slice(&0u32.to_le_bytes());

    let mut key_data = Vec::new();
    for k in &keys {
        let off = key_data.len() as u32;
        key_pool.extend_from_slice(&off.to_le_bytes());
        let b = k.as_bytes();
        key_data.push(b.len() as u8);
        key_data.push(b.len() as u8);
        key_data.extend_from_slice(b);
        key_data.push(0);
    }
    key_pool.extend_from_slice(&key_data);
    while key_pool.len() % 4 != 0 {
        key_pool.push(0);
    }
    let key_pool_sz = key_pool.len() as u32;
    key_pool[4..8].copy_from_slice(&key_pool_sz.to_le_bytes());
    pkg_chunk.extend_from_slice(&key_pool);

    // 3. Type chunk for String entries (Type ID = 4)
    if !strings.is_empty() {
        let type_start = pkg_chunk.len();
        let hdr_size = 52u16;
        pkg_chunk.extend_from_slice(&RES_TABLE_TYPE_TYPE.to_le_bytes());
        pkg_chunk.extend_from_slice(&hdr_size.to_le_bytes());
        pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // placeholder
        pkg_chunk.push(4u8); // type ID 4 = "string"
        pkg_chunk.push(0);
        pkg_chunk.extend_from_slice(&0u16.to_le_bytes());
        pkg_chunk.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        let entries_start = 52 + strings.len() * 4;
        pkg_chunk.extend_from_slice(&(entries_start as u32).to_le_bytes());
        pkg_chunk.extend_from_slice(&[0u8; 32]); // config (any)

        let mut entry_data = Vec::new();
        for (idx, _) in strings.iter().enumerate() {
            let off = entry_data.len() as u32;
            pkg_chunk.extend_from_slice(&off.to_le_bytes());

            // ResTable_entry (8 bytes) + Res_value (8 bytes) = 16 bytes
            entry_data.extend_from_slice(&8u16.to_le_bytes()); // size = 8
            entry_data.extend_from_slice(&0u16.to_le_bytes()); // flags = 0
            entry_data.extend_from_slice(&(idx as u32).to_le_bytes()); // key index

            // Res_value
            entry_data.extend_from_slice(&8u16.to_le_bytes()); // size
            entry_data.push(0); // res0
            entry_data.push(TYPE_STRING);
            entry_data.extend_from_slice(&(idx as u32).to_le_bytes()); // string pool index
        }
        pkg_chunk.extend_from_slice(&entry_data);
        while (pkg_chunk.len() - type_start) % 4 != 0 {
            pkg_chunk.push(0);
        }
        let type_sz = (pkg_chunk.len() - type_start) as u32;
        pkg_chunk[type_start + 4..type_start + 8].copy_from_slice(&type_sz.to_le_bytes());
    }

    // 4. Type chunk for Color entries (Type ID = 5)
    if !colors.is_empty() {
        let type_start = pkg_chunk.len();
        let hdr_size = 52u16;
        pkg_chunk.extend_from_slice(&RES_TABLE_TYPE_TYPE.to_le_bytes());
        pkg_chunk.extend_from_slice(&hdr_size.to_le_bytes());
        pkg_chunk.extend_from_slice(&0u32.to_le_bytes());
        pkg_chunk.push(5u8); // type ID 5 = "color"
        pkg_chunk.push(0);
        pkg_chunk.extend_from_slice(&0u16.to_le_bytes());
        pkg_chunk.extend_from_slice(&(colors.len() as u32).to_le_bytes());
        let entries_start = 52 + colors.len() * 4;
        pkg_chunk.extend_from_slice(&(entries_start as u32).to_le_bytes());
        pkg_chunk.extend_from_slice(&[0u8; 32]);

        let mut entry_data = Vec::new();
        let key_base = strings.len();
        for (idx, (_, argb)) in colors.iter().enumerate() {
            let off = entry_data.len() as u32;
            pkg_chunk.extend_from_slice(&off.to_le_bytes());

            entry_data.extend_from_slice(&8u16.to_le_bytes());
            entry_data.extend_from_slice(&0u16.to_le_bytes());
            let k_idx = (key_base + idx) as u32;
            entry_data.extend_from_slice(&k_idx.to_le_bytes());

            entry_data.extend_from_slice(&8u16.to_le_bytes());
            entry_data.push(0);
            entry_data.push(TYPE_INT_COLOR_ARGB8);
            entry_data.extend_from_slice(&argb.to_le_bytes());
        }
        pkg_chunk.extend_from_slice(&entry_data);
        while (pkg_chunk.len() - type_start) % 4 != 0 {
            pkg_chunk.push(0);
        }
        let type_sz = (pkg_chunk.len() - type_start) as u32;
        pkg_chunk[type_start + 4..type_start + 8].copy_from_slice(&type_sz.to_le_bytes());
    }

    let pkg_sz = pkg_chunk.len() as u32;
    pkg_chunk[4..8].copy_from_slice(&pkg_sz.to_le_bytes());

    // 5. Build Final Root Table Chunk: [RES_TABLE_TYPE: 2][hdr: 2][total_sz: 4][pkg_count: 4]
    let hdr_size = 12u16;
    let total_sz = (12 + str_pool.len() + pkg_chunk.len()) as u32;

    arsc.extend_from_slice(&RES_TABLE_TYPE.to_le_bytes());
    arsc.extend_from_slice(&hdr_size.to_le_bytes());
    arsc.extend_from_slice(&total_sz.to_le_bytes());
    arsc.extend_from_slice(&1u32.to_le_bytes()); // 1 package
    arsc.extend_from_slice(&str_pool);
    arsc.extend_from_slice(&pkg_chunk);

    arsc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resources_arsc_generation_and_parsing() {
        let arsc_data = create_test_resources_arsc(
            "org.whispersystems.signal",
            &[("app_name", "Signal"), ("welcome_msg", "Hello InterOS")],
            &[("primary_color", 0xFF2C6BED), ("accent_color", 0xFF00FF00)],
        );

        let table = ResourceTable::parse(&arsc_data).expect("parse valid resources.arsc");
        assert_eq!(table.packages.len(), 1);
        let pkg = table.packages.values().next().expect("package present");
        assert_eq!(pkg.name, "org.whispersystems.signal");

        let res_id_app_name = 0x7F040000;
        let val_app_name = table.get_value(res_id_app_name).expect("resolve app_name");
        assert_eq!(val_app_name, &ResourceValue::String(String::from("Signal")));
        assert_eq!(table.get_string(res_id_app_name), Some("Signal"));

        let res_id_welcome = 0x7F040001;
        assert_eq!(table.get_string(res_id_welcome), Some("Hello InterOS"));

        let res_id_primary = 0x7F050000;
        let val_primary = table.get_value(res_id_primary).expect("resolve primary_color");
        assert_eq!(val_primary, &ResourceValue::Color(0xFF2C6BED));
        assert_eq!(table.get_color(res_id_primary), Some(0xFF2C6BED));
    }

    #[test]
    fn test_resources_arsc_bounds_and_error_handling() {
        assert_eq!(ResourceTable::parse(&[]).unwrap_err(), ResourceError::TooSmall);
        assert_eq!(ResourceTable::parse(&[0u8; 10]).unwrap_err(), ResourceError::TooSmall);
        assert_eq!(ResourceTable::parse(&[0x05, 0x00, 0x08, 0x00, 0x10, 0x00, 0x00, 0x00, 0, 0, 0, 0]).unwrap_err(), ResourceError::InvalidHeader);
    }
}

