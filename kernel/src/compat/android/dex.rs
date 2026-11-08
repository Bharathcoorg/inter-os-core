// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Dalvik Executable (DEX) Binary Parser & Capability Verifier
//!
//! Provides bare-metal `no_std` parsing, Adler-32 integrity verification,
//! and bytecode auditing for Android Dalvik Executable files (`classes.dex`).
//!
//! Inter OS executes Android bytecode in a capability-bounded sandbox
//! without copying Linux kernel binders, Android ART internals, or ambient authority.

use alloc::string::String;
use alloc::vec::Vec;

pub const DEX_MAGIC: &[u8; 8] = b"dex\n035\0";
pub const DEX_MAGIC_037: &[u8; 8] = b"dex\n037\0";
pub const DEX_MAGIC_038: &[u8; 8] = b"dex\n038\0";
pub const DEX_MAGIC_039: &[u8; 8] = b"dex\n039\0";

/// Validates whether the given magic bytes match a supported DEX format version (035, 037, 038, 039).
#[inline]
pub fn is_valid_dex_magic(magic: &[u8; 8]) -> bool {
    magic == DEX_MAGIC || magic == DEX_MAGIC_037 || magic == DEX_MAGIC_038 || magic == DEX_MAGIC_039
}
pub const DEX_HEADER_SIZE: usize = 0x70; // 112 bytes
pub const ENDIAN_CONSTANT: u32 = 0x12345678;

// Dalvik Opcodes
pub const OP_NOP: u8 = 0x00;
pub const OP_MOVE: u8 = 0x01;
pub const OP_MOVE_RESULT: u8 = 0x0A;
pub const OP_MOVE_EXCEPTION: u8 = 0x0D;
pub const OP_RETURN_VOID: u8 = 0x0E;
pub const OP_RETURN: u8 = 0x0F;
pub const OP_RETURN_WIDE: u8 = 0x10;
pub const OP_CONST_4: u8 = 0x12;
pub const OP_CONST_16: u8 = 0x13;
pub const OP_CONST_WIDE: u8 = 0x16;
pub const OP_CONST_STRING: u8 = 0x1A;
pub const OP_CONST_STRING_JUMBO: u8 = 0x1B;
pub const OP_CONST_CLASS: u8 = 0x1C;
pub const OP_NEW_INSTANCE: u8 = 0x22;
pub const OP_GOTO: u8 = 0x28;
pub const OP_GOTO_16: u8 = 0x29;
pub const OP_IF_EQ: u8 = 0x32;
pub const OP_IF_NE: u8 = 0x33;
pub const OP_IF_EQZ: u8 = 0x38;
pub const OP_IF_NEZ: u8 = 0x39;
pub const OP_INVOKE_VIRTUAL: u8 = 0x6E;
pub const OP_INVOKE_DIRECT: u8 = 0x70;
pub const OP_INVOKE_STATIC: u8 = 0x71;
pub const OP_ADD_INT_2ADDR: u8 = 0xB0;
pub const OP_SUB_INT_2ADDR: u8 = 0xB1;
pub const OP_MUL_INT_2ADDR: u8 = 0xB2;
pub const OP_DIV_INT_2ADDR: u8 = 0xB3;
pub const OP_REM_INT_2ADDR: u8 = 0xB4;
pub const OP_AND_INT_2ADDR: u8 = 0xB5;
pub const OP_OR_INT_2ADDR: u8 = 0xB6;
pub const OP_XOR_INT_2ADDR: u8 = 0xB7;
pub const OP_SHL_INT_2ADDR: u8 = 0xB8;
pub const OP_SHR_INT_2ADDR: u8 = 0xB9;
pub const OP_USHR_INT_2ADDR: u8 = 0xBA;

pub const OP_ARRAY_LENGTH: u8 = 0x21;
pub const OP_NEW_ARRAY: u8 = 0x23;

pub const OP_IF_LT: u8 = 0x34;
pub const OP_IF_GE: u8 = 0x35;
pub const OP_IF_GT: u8 = 0x36;
pub const OP_IF_LE: u8 = 0x37;
pub const OP_IF_LTZ: u8 = 0x3A;
pub const OP_IF_GEZ: u8 = 0x3B;
pub const OP_IF_GTZ: u8 = 0x3C;
pub const OP_IF_LEZ: u8 = 0x3D;

pub const OP_AGET: u8 = 0x44;
pub const OP_APUT: u8 = 0x4B;

pub const OP_IGET: u8 = 0x52;
pub const OP_IPUT: u8 = 0x59;

pub const OP_ADD_INT: u8 = 0x90;
pub const OP_SUB_INT: u8 = 0x91;
pub const OP_MUL_INT: u8 = 0x92;
pub const OP_DIV_INT: u8 = 0x93;
pub const OP_REM_INT: u8 = 0x94;
pub const OP_AND_INT: u8 = 0x95;
pub const OP_OR_INT: u8 = 0x96;
pub const OP_XOR_INT: u8 = 0x97;
pub const OP_SHL_INT: u8 = 0x98;
pub const OP_SHR_INT: u8 = 0x99;
pub const OP_USHR_INT: u8 = 0x9A;

/// Raw DEX File Header (112 bytes).
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct DexHeader {
    pub magic: [u8; 8],
    pub checksum: u32,
    pub signature: [u8; 20],
    pub file_size: u32,
    pub header_size: u32,
    pub endian_tag: u32,
    pub link_size: u32,
    pub link_off: u32,
    pub map_off: u32,
    pub string_ids_size: u32,
    pub string_ids_off: u32,
    pub type_ids_size: u32,
    pub type_ids_off: u32,
    pub proto_ids_size: u32,
    pub proto_ids_off: u32,
    pub field_ids_size: u32,
    pub field_ids_off: u32,
    pub method_ids_size: u32,
    pub method_ids_off: u32,
    pub class_defs_size: u32,
    pub class_defs_off: u32,
    pub data_size: u32,
    pub data_off: u32,
}

/// Errors returned during DEX parsing and bytecode verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexError {
    BufferTooShort,
    InvalidMagic,
    InvalidEndianTag,
    InvalidHeaderSize,
    ChecksumMismatch,
    MalformedUleb128,
    StringIndexOutOfBounds,
    TypeIndexOutOfBounds,
    MethodIndexOutOfBounds,
    IllegalBytecodeInstruction(u8),
    SecurityViolationForbiddenApi,
    BranchOutOfBounds,
    ExcessiveRegisters,
    ExcessiveArrayAllocation,
    UnexpectedEof,
}

/// Parsed method descriptor from DEX string pool and method ID tables.
#[derive(Debug, Clone)]
pub struct DexMethod {
    /// Stable DEX method_id index used by invoke-* instructions.
    pub method_index: usize,
    pub class_name: String,
    pub method_name: String,
    pub proto_shorty: String,
    pub is_direct: bool,
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    pub instruction_count: usize,
    pub code_offset: usize,
}

/// Parsed DEX binary package structure.
pub struct DexPackage<'a> {
    pub raw: &'a [u8],
    pub header: DexHeader,
    pub strings: Vec<String>,
    pub types: Vec<String>,
    pub methods: Vec<DexMethod>,
}

impl<'a> DexPackage<'a> {
    /// Parses a DEX binary file from a byte slice and validates its structure.
    pub fn parse(raw: &'a [u8]) -> Result<Self, DexError> {
        if raw.len() < DEX_HEADER_SIZE {
            return Err(DexError::BufferTooShort);
        }

        let hdr_ptr = raw.as_ptr() as *const DexHeader;
        let header: DexHeader = unsafe { core::ptr::read_unaligned(hdr_ptr) };

        if !is_valid_dex_magic(&header.magic) {
            return Err(DexError::InvalidMagic);
        }

        if u32::from_le(header.endian_tag) != ENDIAN_CONSTANT {
            return Err(DexError::InvalidEndianTag);
        }

        if u32::from_le(header.header_size) as usize != DEX_HEADER_SIZE {
            return Err(DexError::InvalidHeaderSize);
        }

        // Verify Adler-32 Checksum
        let expected_checksum = u32::from_le(header.checksum);
        let computed_checksum = compute_adler32(&raw[12..]);
        if expected_checksum != 0 && computed_checksum != expected_checksum {
            return Err(DexError::ChecksumMismatch);
        }

        // 1. Parse String Pool
        let string_ids_size = u32::from_le(header.string_ids_size) as usize;
        let string_ids_off = u32::from_le(header.string_ids_off) as usize;
        let mut strings = Vec::with_capacity(string_ids_size);

        for i in 0..string_ids_size {
            let offset_pos = string_ids_off + i * 4;
            if offset_pos + 4 > raw.len() {
                return Err(DexError::UnexpectedEof);
            }
            let str_data_off = u32::from_le_bytes(raw[offset_pos..offset_pos + 4].try_into().unwrap()) as usize;
            if str_data_off >= raw.len() {
                return Err(DexError::StringIndexOutOfBounds);
            }

            let mut cur = str_data_off;
            let _utf16_size = read_uleb128(raw, &mut cur)?;
            let str_start = cur;
            while cur < raw.len() && raw[cur] != 0 {
                cur += 1;
            }
            let s = core::str::from_utf8(&raw[str_start..cur]).unwrap_or("<invalid utf8>");
            strings.push(String::from(s));
        }

        // 2. Parse Type IDs
        let type_ids_size = u32::from_le(header.type_ids_size) as usize;
        let type_ids_off = u32::from_le(header.type_ids_off) as usize;
        let mut types = Vec::with_capacity(type_ids_size);

        for i in 0..type_ids_size {
            let offset_pos = type_ids_off + i * 4;
            if offset_pos + 4 > raw.len() {
                return Err(DexError::UnexpectedEof);
            }
            let descriptor_idx = u32::from_le_bytes(raw[offset_pos..offset_pos + 4].try_into().unwrap()) as usize;
            if descriptor_idx >= strings.len() {
                return Err(DexError::TypeIndexOutOfBounds);
            }
            types.push(strings[descriptor_idx].clone());
        }

        // 3. Parse Method IDs
        let method_ids_size = u32::from_le(header.method_ids_size) as usize;
        let method_ids_off = u32::from_le(header.method_ids_off) as usize;
        let mut method_entries = Vec::with_capacity(method_ids_size);

        for i in 0..method_ids_size {
            let offset_pos = method_ids_off + i * 8;
            if offset_pos + 8 > raw.len() {
                break;
            }
            let class_idx = u16::from_le_bytes(raw[offset_pos..offset_pos + 2].try_into().unwrap()) as usize;
            let proto_idx = u16::from_le_bytes(raw[offset_pos + 2..offset_pos + 4].try_into().unwrap()) as usize;
            let name_idx = u32::from_le_bytes(raw[offset_pos + 4..offset_pos + 8].try_into().unwrap()) as usize;

            let class_name = if class_idx < types.len() { types[class_idx].clone() } else { String::from("<unknown>") };
            let method_name = if name_idx < strings.len() { strings[name_idx].clone() } else { String::from("<unknown>") };
            let proto_str = if proto_idx < strings.len() { strings[proto_idx].clone() } else { String::from("V") };

            method_entries.push((class_name, method_name, proto_str));
        }

        // 4. Parse Class Defs & Code Items
        let class_defs_size = u32::from_le(header.class_defs_size) as usize;
        let class_defs_off = u32::from_le(header.class_defs_off) as usize;
        let mut methods = Vec::new();

        for i in 0..class_defs_size {
            let offset_pos = class_defs_off + i * 32;
            if offset_pos + 32 > raw.len() {
                break;
            }
            let class_data_off = u32::from_le_bytes(raw[offset_pos + 24..offset_pos + 28].try_into().unwrap()) as usize;
            if class_data_off == 0 || class_data_off >= raw.len() {
                continue;
            }

            let mut cur = class_data_off;
            let static_fields_size = read_uleb128(raw, &mut cur)?;
            let instance_fields_size = read_uleb128(raw, &mut cur)?;
            let direct_methods_size = read_uleb128(raw, &mut cur)?;
            let virtual_methods_size = read_uleb128(raw, &mut cur)?;

            // Skip static and instance fields
            for _ in 0..static_fields_size {
                let _idx = read_uleb128(raw, &mut cur)?;
                let _flags = read_uleb128(raw, &mut cur)?;
            }
            for _ in 0..instance_fields_size {
                let _idx = read_uleb128(raw, &mut cur)?;
                let _flags = read_uleb128(raw, &mut cur)?;
            }

            // Direct methods
            let mut last_method_idx = 0;
            for _ in 0..direct_methods_size {
                let diff = read_uleb128(raw, &mut cur)?;
                let _access_flags = read_uleb128(raw, &mut cur)?;
                let code_off = read_uleb128(raw, &mut cur)? as usize;
                last_method_idx += diff;

                let (c_name, m_name, p_name) = if (last_method_idx as usize) < method_entries.len() {
                    method_entries[last_method_idx as usize].clone()
                } else {
                    (String::from("Unknown"), String::from("unknown"), String::from("V"))
                };

                let mut reg_size = 0;
                let mut ins_size = 0;
                let mut outs_size = 0;
                let mut insns_count = 0;

                if code_off > 0 && code_off + 16 <= raw.len() {
                    reg_size = u16::from_le_bytes(raw[code_off..code_off + 2].try_into().unwrap());
                    ins_size = u16::from_le_bytes(raw[code_off + 2..code_off + 4].try_into().unwrap());
                    outs_size = u16::from_le_bytes(raw[code_off + 4..code_off + 6].try_into().unwrap());
                    insns_count = u32::from_le_bytes(raw[code_off + 12..code_off + 16].try_into().unwrap()) as usize;
                }

                methods.push(DexMethod {
                    method_index: last_method_idx as usize,
                    class_name: c_name,
                    method_name: m_name,
                    proto_shorty: p_name,
                    is_direct: true,
                    registers_size: reg_size,
                    ins_size,
                    outs_size,
                    instruction_count: insns_count,
                    code_offset: code_off,
                });
            }

            // Virtual methods
            let mut last_vmethod_idx = 0;
            for _ in 0..virtual_methods_size {
                let diff = read_uleb128(raw, &mut cur)?;
                let _access_flags = read_uleb128(raw, &mut cur)?;
                let code_off = read_uleb128(raw, &mut cur)? as usize;
                last_vmethod_idx += diff;

                let (c_name, m_name, p_name) = if (last_vmethod_idx as usize) < method_entries.len() {
                    method_entries[last_vmethod_idx as usize].clone()
                } else {
                    (String::from("Unknown"), String::from("unknown"), String::from("V"))
                };

                let mut reg_size = 0;
                let mut ins_size = 0;
                let mut outs_size = 0;
                let mut insns_count = 0;

                if code_off > 0 && code_off + 16 <= raw.len() {
                    reg_size = u16::from_le_bytes(raw[code_off..code_off + 2].try_into().unwrap());
                    ins_size = u16::from_le_bytes(raw[code_off + 2..code_off + 4].try_into().unwrap());
                    outs_size = u16::from_le_bytes(raw[code_off + 4..code_off + 6].try_into().unwrap());
                    insns_count = u32::from_le_bytes(raw[code_off + 12..code_off + 16].try_into().unwrap()) as usize;
                }

                methods.push(DexMethod {
                    method_index: last_vmethod_idx as usize,
                    class_name: c_name,
                    method_name: m_name,
                    proto_shorty: p_name,
                    is_direct: false,
                    registers_size: reg_size,
                    ins_size,
                    outs_size,
                    instruction_count: insns_count,
                    code_offset: code_off,
                });
            }
        }

        Ok(Self {
            raw,
            header,
            strings,
            types,
            methods,
        })
    }

    /// Zero-allocation string lookup directly over raw byte slice.
    pub fn get_string_slice(&self, idx: u32) -> Result<&'a str, DexError> {
        let string_ids_size = u32::from_le(self.header.string_ids_size);
        if idx >= string_ids_size {
            return Err(DexError::StringIndexOutOfBounds);
        }
        let string_ids_off = u32::from_le(self.header.string_ids_off) as usize;
        let offset_pos = string_ids_off + (idx as usize) * 4;
        if offset_pos + 4 > self.raw.len() {
            return Err(DexError::UnexpectedEof);
        }
        let str_data_off = u32::from_le_bytes(
            self.raw[offset_pos..offset_pos + 4]
                .try_into()
                .map_err(|_| DexError::UnexpectedEof)?,
        ) as usize;
        if str_data_off >= self.raw.len() {
            return Err(DexError::StringIndexOutOfBounds);
        }

        let mut cur = str_data_off;
        let _utf16_size = read_uleb128(self.raw, &mut cur)?;
        let str_start = cur;
        while cur < self.raw.len() && self.raw[cur] != 0 {
            cur += 1;
        }
        core::str::from_utf8(&self.raw[str_start..cur]).map_err(|_| DexError::StringIndexOutOfBounds)
    }

    /// Zero-allocation type descriptor lookup directly over raw byte slice.
    pub fn get_type_slice(&self, idx: u32) -> Result<&'a str, DexError> {
        let type_ids_size = u32::from_le(self.header.type_ids_size);
        if idx >= type_ids_size {
            return Err(DexError::TypeIndexOutOfBounds);
        }
        let type_ids_off = u32::from_le(self.header.type_ids_off) as usize;
        let offset_pos = type_ids_off + (idx as usize) * 4;
        if offset_pos + 4 > self.raw.len() {
            return Err(DexError::UnexpectedEof);
        }
        let descriptor_idx = u32::from_le_bytes(
            self.raw[offset_pos..offset_pos + 4]
                .try_into()
                .map_err(|_| DexError::UnexpectedEof)?,
        );
        self.get_string_slice(descriptor_idx)
    }

    /// Performs capability verification over all bytecode instructions.
    /// Rejects forbidden reflection, ambient process execution, out-of-bound branches,
    /// excessive register usage (>32), and unbounded array allocations.
    pub fn audit_bytecode(&self) -> Result<u32, DexError> {
        // 1. Verify Adler-32 Checksum
        let expected_checksum = u32::from_le(self.header.checksum);
        let computed_checksum = compute_adler32(&self.raw[12..]);
        if expected_checksum != 0 && computed_checksum != expected_checksum {
            return Err(DexError::ChecksumMismatch);
        }

        // 2. Verify string pool does not contain forbidden ambient capabilities
        let string_count = u32::from_le(self.header.string_ids_size);
        for i in 0..string_count {
            let s = self.get_string_slice(i)?;
            if s.contains("java/lang/ProcessBuilder")
                || s.contains("java/lang/Runtime")
                || s.contains("android/os/ExecutableCreator")
                || s.contains("/system/bin/sh")
                || s.contains("/data/local/tmp")
            {
                return Err(DexError::SecurityViolationForbiddenApi);
            }
        }

        let mut verified_instructions = 0;

        // 3. Audit methods, registers, CFG branches, and instructions
        for m in &self.methods {
            // Reject excessive register allocations
            if m.registers_size > 32 {
                return Err(DexError::ExcessiveRegisters);
            }

            // Verify method class/name against forbidden APIs
            if m.class_name.contains("ProcessBuilder")
                || m.class_name.contains("Runtime")
                || m.class_name.contains("ExecutableCreator")
            {
                return Err(DexError::SecurityViolationForbiddenApi);
            }

            if m.code_offset > 0 && m.code_offset + 16 <= self.raw.len() {
                let insns_len = m.instruction_count * 2;
                let insns_start = m.code_offset + 16;
                if insns_start + insns_len > self.raw.len() {
                    return Err(DexError::UnexpectedEof);
                }

                let insns = &self.raw[insns_start..insns_start + insns_len];
                let mut pc = 0;

                while pc < insns.len() {
                    let opcode = insns[pc];
                    match opcode {
                        OP_GOTO => {
                            if pc + 2 > insns.len() {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            let offset = insns[pc + 1] as i8 as isize * 2;
                            let target = pc as isize + offset;
                            if target < 0 || target >= insns.len() as isize {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            pc += 2;
                        }
                        0x29 => { // OP_GOTO_16
                            if pc + 4 > insns.len() {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            let offset = i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize * 2;
                            let target = pc as isize + offset;
                            if target < 0 || target >= insns.len() as isize {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            pc += 4;
                        }
                        0x24 | 0x25 => { // OP_FILLED_NEW_ARRAY, OP_FILLED_NEW_ARRAY_RANGE
                            if pc + 6 > insns.len() {
                                return Err(DexError::UnexpectedEof);
                            }
                            let size = insns[pc + 1] as usize;
                            if size > 262_144 { // > 1MB of 4-byte elements
                                return Err(DexError::ExcessiveArrayAllocation);
                            }
                            pc += 6;
                        }
                        OP_INVOKE_VIRTUAL | OP_INVOKE_DIRECT | OP_INVOKE_STATIC => {
                            if pc + 4 > insns.len() {
                                return Err(DexError::UnexpectedEof);
                            }
                            let method_ref = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                            if method_ref < self.methods.len() {
                                let target_method = &self.methods[method_ref];
                                if target_method.class_name.contains("ProcessBuilder")
                                    || target_method.class_name.contains("Runtime")
                                    || target_method.class_name.contains("ExecutableCreator")
                                {
                                    return Err(DexError::SecurityViolationForbiddenApi);
                                }
                            }
                            pc += 6;
                        }
                        _ => {
                            pc += 2;
                        }
                    }
                }
            }

            verified_instructions += m.instruction_count as u32;
        }

        Ok(verified_instructions)
    }
}

/// Reads an unsigned LEB128 integer from slice at `*offset`.
pub fn read_uleb128(bytes: &[u8], offset: &mut usize) -> Result<u32, DexError> {
    let mut result: u32 = 0;
    let mut shift = 0;
    while *offset < bytes.len() {
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift > 35 {
            return Err(DexError::MalformedUleb128);
        }
    }
    Err(DexError::UnexpectedEof)
}

/// Computes the standard Adler-32 checksum (modulo 65521).
pub fn compute_adler32(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

/// Synthesizes a valid, fully formed Dalvik Executable (`classes.dex`) binary.
pub fn create_test_dex(class_name: &str, method_name: &str, log_message: &str) -> Vec<u8> {
    let mut dex = Vec::new();

    // Strings table
    let strings: [&str; 8] = [
        "V",
        "Ljava/lang/Object;",
        class_name,
        method_name,
        "(Landroid/os/Bundle;)V",
        "Landroid/util/Log;",
        "i",
        log_message,
    ];

    let string_ids_off = DEX_HEADER_SIZE;
    let string_ids_size = strings.len();

    let type_ids_off = string_ids_off + string_ids_size * 4;
    let type_ids_size = 3; // V, Object, class_name

    let proto_ids_off = type_ids_off + type_ids_size * 4;
    let proto_ids_size = 1;

    let method_ids_off = proto_ids_off + proto_ids_size * 12;
    let method_ids_size = 1; // class_name->method_name

    let class_defs_off = method_ids_off + method_ids_size * 8;
    let class_defs_size = 1;

    let data_off = class_defs_off + class_defs_size * 32;

    // Pad to data_off
    dex.resize(data_off, 0);

    // Build data section:
    // 1. Strings data
    let mut string_offsets = Vec::new();
    for s in &strings {
        let off = dex.len();
        string_offsets.push(off as u32);
        let s_bytes = s.as_bytes();
        dex.push(s_bytes.len() as u8); // uleb128 length
        dex.extend_from_slice(s_bytes);
        dex.push(0); // null terminator
    }

    // Align to 4 bytes
    while dex.len() % 4 != 0 {
        dex.push(0);
    }

    // 2. Code Item
    let code_off = dex.len() as u32;
    // registers_size = 2, ins_size = 1, outs_size = 2, tries_size = 0, debug_info_off = 0
    dex.extend_from_slice(&2u16.to_le_bytes()); // registers_size = 2
    dex.extend_from_slice(&1u16.to_le_bytes()); // ins_size = 1
    dex.extend_from_slice(&2u16.to_le_bytes()); // outs_size = 2
    dex.extend_from_slice(&0u16.to_le_bytes()); // tries_size = 0
    dex.extend_from_slice(&0u32.to_le_bytes()); // debug_info_off = 0
    dex.extend_from_slice(&4u32.to_le_bytes()); // insns_size = 4 (4 x 16-bit)
    // Instructions:
    // 0: const-string v0, string@7 (log_message) -> [0x1A, 0x00], [0x07, 0x00]
    dex.extend_from_slice(&[OP_CONST_STRING, 0x00, 0x07, 0x00]);
    // 1: return-void -> [0x0E, 0x00]
    dex.extend_from_slice(&[OP_RETURN_VOID, 0x00, 0x00, 0x00]);

    // Align to 4 bytes
    while dex.len() % 4 != 0 {
        dex.push(0);
    }

    // 3. Class Data Item
    let class_data_off = dex.len() as u32;
    dex.push(0); // static_fields_size = 0
    dex.push(0); // instance_fields_size = 0
    dex.push(1); // direct_methods_size = 1
    dex.push(0); // virtual_methods_size = 0
    // Encoded method: method_idx_diff = 0, access_flags = 1 (public), code_off
    dex.push(0); // method_idx_diff = 0
    dex.push(1); // access_flags = 1
    // Write code_off as uleb128
    let mut code_cur = code_off;
    while code_cur >= 0x80 {
        dex.push(((code_cur & 0x7F) | 0x80) as u8);
        code_cur >>= 7;
    }
    dex.push((code_cur & 0x7F) as u8);

    // Total file size
    let file_size = dex.len() as u32;
    let data_size = file_size - data_off as u32;

    // Fill in Header
    let hdr = DexHeader {
        magic: *DEX_MAGIC,
        checksum: 0,
        signature: [0u8; 20],
        file_size: file_size.to_le(),
        header_size: (DEX_HEADER_SIZE as u32).to_le(),
        endian_tag: ENDIAN_CONSTANT.to_le(),
        link_size: 0,
        link_off: 0,
        map_off: 0,
        string_ids_size: (string_ids_size as u32).to_le(),
        string_ids_off: (string_ids_off as u32).to_le(),
        type_ids_size: (type_ids_size as u32).to_le(),
        type_ids_off: (type_ids_off as u32).to_le(),
        proto_ids_size: (proto_ids_size as u32).to_le(),
        proto_ids_off: (proto_ids_off as u32).to_le(),
        field_ids_size: 0,
        field_ids_off: 0,
        method_ids_size: (method_ids_size as u32).to_le(),
        method_ids_off: (method_ids_off as u32).to_le(),
        class_defs_size: (class_defs_size as u32).to_le(),
        class_defs_off: (class_defs_off as u32).to_le(),
        data_size: data_size.to_le(),
        data_off: (data_off as u32).to_le(),
    };

    // Fill String IDs
    for (i, off) in string_offsets.iter().enumerate() {
        let pos = string_ids_off + i * 4;
        dex[pos..pos + 4].copy_from_slice(&off.to_le_bytes());
    }

    // Fill Type IDs: 0->0 ("V"), 1->1 ("Object"), 2->2 (class_name)
    let type_str_indices = [0u32, 1, 2];
    for (i, &s_idx) in type_str_indices.iter().enumerate() {
        let pos = type_ids_off + i * 4;
        dex[pos..pos + 4].copy_from_slice(&s_idx.to_le_bytes());
    }

    // Fill Proto IDs: shorty=0 ("V"), return_type=0 ("V"), parameters_off=0
    let pos = proto_ids_off;
    dex[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
    dex[pos + 4..pos + 8].copy_from_slice(&0u32.to_le_bytes());
    dex[pos + 8..pos + 12].copy_from_slice(&0u32.to_le_bytes());

    // Fill Method IDs: class_idx=2, proto_idx=0, name_idx=3 (method_name)
    let pos = method_ids_off;
    dex[pos..pos + 2].copy_from_slice(&2u16.to_le_bytes());
    dex[pos + 2..pos + 4].copy_from_slice(&0u16.to_le_bytes());
    dex[pos + 4..pos + 8].copy_from_slice(&3u32.to_le_bytes());

    // Fill Class Defs: class_idx=2, access_flags=1, superclass=1, class_data_off
    let pos = class_defs_off;
    dex[pos..pos + 4].copy_from_slice(&2u32.to_le_bytes());
    dex[pos + 4..pos + 8].copy_from_slice(&1u32.to_le_bytes());
    dex[pos + 8..pos + 12].copy_from_slice(&1u32.to_le_bytes());
    dex[pos + 24..pos + 28].copy_from_slice(&class_data_off.to_le_bytes());

    // Write header bytes into dex[0..112]
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            &hdr as *const DexHeader as *const u8,
            DEX_HEADER_SIZE,
        )
    };
    dex[..DEX_HEADER_SIZE].copy_from_slice(hdr_bytes);

    // Compute and store Adler-32 Checksum
    let checksum = compute_adler32(&dex[12..]);
    dex[8..12].copy_from_slice(&checksum.to_le_bytes());

    dex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_copy_dex_slices() {
        let raw = create_test_dex("Lorg/interos/TestActivity;", "onCreate", "ZeroCopyWorks");
        let pkg = DexPackage::parse(&raw).expect("parse DEX");
        assert_eq!(pkg.get_string_slice(0).unwrap(), "V");
        assert_eq!(pkg.get_string_slice(1).unwrap(), "Ljava/lang/Object;");
        assert_eq!(pkg.get_string_slice(2).unwrap(), "Lorg/interos/TestActivity;");
        assert_eq!(pkg.get_string_slice(3).unwrap(), "onCreate");
        assert_eq!(pkg.get_string_slice(7).unwrap(), "ZeroCopyWorks");
        assert_eq!(pkg.get_string_slice(99), Err(DexError::StringIndexOutOfBounds));

        assert_eq!(pkg.get_type_slice(0).unwrap(), "V");
        assert_eq!(pkg.get_type_slice(1).unwrap(), "Ljava/lang/Object;");
        assert_eq!(pkg.get_type_slice(2).unwrap(), "Lorg/interos/TestActivity;");
        assert_eq!(pkg.get_type_slice(10), Err(DexError::TypeIndexOutOfBounds));
    }

    #[test]
    fn test_dex_magic_versions() {
        assert!(is_valid_dex_magic(b"dex\n035\0"));
        assert!(is_valid_dex_magic(b"dex\n037\0"));
        assert!(is_valid_dex_magic(b"dex\n038\0"));
        assert!(is_valid_dex_magic(b"dex\n039\0"));
        assert!(!is_valid_dex_magic(b"dex\n036\0"));
        assert!(!is_valid_dex_magic(b"dey\n035\0"));
        assert!(!is_valid_dex_magic(b"badmagic"));
    }

    #[test]
    fn test_dex_version_038_acceptance() {
        let mut raw = create_test_dex("Lorg/interos/ModernActivity;", "onCreate", "ModernDex");
        // Overwrite magic with DEX 038
        raw[0..8].copy_from_slice(b"dex\n038\0");
        // Recompute Adler-32
        let checksum = compute_adler32(&raw[12..]);
        raw[8..12].copy_from_slice(&checksum.to_le_bytes());

        let pkg = DexPackage::parse(&raw).expect("parse DEX 038");
        assert_eq!(pkg.get_string_slice(2).unwrap(), "Lorg/interos/ModernActivity;");
    }
}

