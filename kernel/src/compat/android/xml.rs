// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! General Android Binary XML (AXML) Parser
//!
//! Decodes compiled Android Binary XML files (layout XMLs, drawable XMLs, menu XMLs)
//! into structured in-memory element trees without string fabrication or ambient authority.
//!
//! Complies with AOSP ResXMLTree specification:
//! - RES_XML_TYPE (0x0003)
//! - RES_STRING_POOL_TYPE (0x0001)
//! - RES_XML_RESOURCE_MAP_TYPE (0x0180)
//! - RES_XML_START_ELEMENT_TYPE (0x0102)
//! - RES_XML_END_ELEMENT_TYPE (0x0103)
//! - RES_XML_CDATA_TYPE (0x0104)

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::resources::{StringPool, RES_XML_TYPE};

pub const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;
pub const RES_XML_START_NAMESPACE_TYPE: u16 = 0x0100;
pub const RES_XML_END_NAMESPACE_TYPE: u16 = 0x0101;
pub const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
pub const RES_XML_END_ELEMENT_TYPE: u16 = 0x0103;
pub const RES_XML_CDATA_TYPE: u16 = 0x0104;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxmlError {
    TooSmall,
    InvalidHeader,
    MalformedChunk,
    MalformedStringPool,
    UnexpectedEof,
    UnbalancedTags,
}

/// A parsed attribute in an XML element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxmlAttribute {
    pub namespace: Option<String>,
    pub name: String,
    pub raw_value: Option<String>,
    pub data_type: u8,
    pub data_value: u32,
    pub resource_id: u32,
}

/// An XML node element representing a layout container or widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxmlElement {
    pub tag: String,
    pub namespace: Option<String>,
    pub attributes: BTreeMap<String, AxmlAttribute>,
    pub children: Vec<AxmlElement>,
}

impl AxmlElement {
    pub fn new(tag: String, namespace: Option<String>) -> Self {
        Self {
            tag,
            namespace,
            attributes: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    pub fn get_attr(&self, name: &str) -> Option<&AxmlAttribute> {
        self.attributes.get(name)
    }

    pub fn get_attr_string(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).and_then(|a| a.raw_value.as_deref())
    }

    pub fn get_attr_res_id(&self, name: &str) -> Option<u32> {
        self.attributes.get(name).map(|a| a.data_value)
    }
}

/// A complete decoded Android Binary XML document.
#[derive(Debug, Clone)]
pub struct AxmlDocument {
    pub root: Option<AxmlElement>,
}

impl AxmlDocument {
    /// Parses an in-memory binary XML buffer.
    pub fn parse(data: &[u8]) -> Result<Self, AxmlError> {
        if data.len() < 8 {
            return Err(AxmlError::TooSmall);
        }

        let chunk_type = u16::from_le_bytes(data[0..2].try_into().unwrap());
        if chunk_type != RES_XML_TYPE {
            return Err(AxmlError::InvalidHeader);
        }

        let header_size = u16::from_le_bytes(data[2..4].try_into().unwrap()) as usize;
        let chunk_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;

        if chunk_size > data.len() || header_size < 8 {
            return Err(AxmlError::MalformedChunk);
        }

        // Global String Pool
        let (string_pool, mut cursor) = StringPool::parse(data, header_size)
            .map_err(|_| AxmlError::MalformedStringPool)?;

        let mut res_map = Vec::new();
        let mut element_stack: Vec<AxmlElement> = Vec::new();
        let mut root = None;

        while cursor < chunk_size {
            if cursor + 8 > chunk_size {
                break;
            }
            let sub_type = u16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
            let sub_size = u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;

            if sub_size == 0 || cursor + sub_size > chunk_size {
                return Err(AxmlError::MalformedChunk);
            }

            match sub_type {
                RES_XML_RESOURCE_MAP_TYPE => {
                    // Resource ID mapping table
                    let count = (sub_size.saturating_sub(8)) / 4;
                    for i in 0..count {
                        let pos = cursor + 8 + i * 4;
                        if pos + 4 <= cursor + sub_size {
                            let res_id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
                            res_map.push(res_id);
                        }
                    }
                }
                RES_XML_START_ELEMENT_TYPE => {
                    if sub_size < 36 {
                        return Err(AxmlError::MalformedChunk);
                    }
                    let ns_idx = u32::from_le_bytes(data[cursor + 16..cursor + 20].try_into().unwrap());
                    let name_idx = u32::from_le_bytes(data[cursor + 20..cursor + 24].try_into().unwrap());
                    let attr_count = u16::from_le_bytes([data[cursor + 28], data[cursor + 29]]) as usize;

                    let tag = string_pool.get(name_idx as usize).unwrap_or("").to_string();
                    let ns = if ns_idx != 0xFFFF_FFFF {
                        string_pool.get(ns_idx as usize).map(|s| s.to_string())
                    } else {
                        None
                    };

                    let mut elem = AxmlElement::new(tag, ns);

                    // Attributes start at offset: cursor + attr_start (offset 24 in start element, 2 bytes)
                    let attr_start = u16::from_le_bytes([data[cursor + 24], data[cursor + 25]]) as usize;
                    let attr_size = u16::from_le_bytes([data[cursor + 26], data[cursor + 27]]) as usize;
                    let mut attr_cur = cursor + attr_start;

                    for _i in 0..attr_count {
                        if attr_cur + 20 > cursor + sub_size {
                            break;
                        }
                        let a_ns_idx = u32::from_le_bytes(data[attr_cur..attr_cur + 4].try_into().unwrap());
                        let a_name_idx = u32::from_le_bytes(data[attr_cur + 4..attr_cur + 8].try_into().unwrap());
                        let a_raw_idx = u32::from_le_bytes(data[attr_cur + 8..attr_cur + 12].try_into().unwrap());
                        let a_data_type = data[attr_cur + 15];
                        let a_data_val = u32::from_le_bytes(data[attr_cur + 16..attr_cur + 20].try_into().unwrap());

                        let a_name = string_pool.get(a_name_idx as usize).unwrap_or("").to_string();
                        let a_ns = if a_ns_idx != 0xFFFF_FFFF {
                            string_pool.get(a_ns_idx as usize).map(|s| s.to_string())
                        } else {
                            None
                        };
                        let a_raw = if a_raw_idx != 0xFFFF_FFFF {
                            string_pool.get(a_raw_idx as usize).map(|s| s.to_string())
                        } else {
                            None
                        };
                        let a_res_id = if (a_name_idx as usize) < res_map.len() {
                            res_map[a_name_idx as usize]
                        } else {
                            0
                        };

                        elem.attributes.insert(
                            a_name.clone(),
                            AxmlAttribute {
                                namespace: a_ns,
                                name: a_name,
                                raw_value: a_raw,
                                data_type: a_data_type,
                                data_value: a_data_val,
                                resource_id: a_res_id,
                            },
                        );

                        attr_cur += if attr_size > 0 { attr_size } else { 20 };
                    }

                    element_stack.push(elem);
                }
                RES_XML_END_ELEMENT_TYPE => {
                    if let Some(finished) = element_stack.pop() {
                        if let Some(parent) = element_stack.last_mut() {
                            parent.children.push(finished);
                        } else {
                            root = Some(finished);
                        }
                    } else {
                        return Err(AxmlError::UnbalancedTags);
                    }
                }
                _ => {}
            }

            cursor += sub_size;
        }

        if !element_stack.is_empty() {
            return Err(AxmlError::UnbalancedTags);
        }

        Ok(Self { root })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_axml_element_and_attributes() {
        let mut elem = AxmlElement::new(String::from("TextView"), None);
        elem.attributes.insert(String::from("text"), AxmlAttribute {
            namespace: None,
            name: String::from("text"),
            raw_value: Some(String::from("Hello World")),
            data_type: 0x03,
            data_value: 0,
            resource_id: 0,
        });
        assert_eq!(elem.get_attr_string("text"), Some("Hello World"));

        let child = AxmlElement::new(String::from("Button"), None);
        elem.children.push(child);
        assert_eq!(elem.children.len(), 1);
        assert_eq!(elem.children[0].tag, "Button");
    }

    #[test]
    fn test_axml_bounds_and_error_handling() {
        assert_eq!(AxmlDocument::parse(&[]).unwrap_err(), AxmlError::TooSmall);
        assert_eq!(AxmlDocument::parse(&[0u8; 4]).unwrap_err(), AxmlError::TooSmall);
        assert_eq!(AxmlDocument::parse(&[0x05, 0x00, 0x08, 0x00, 0x10, 0x00, 0x00, 0x00]).unwrap_err(), AxmlError::InvalidHeader);
    }
}

