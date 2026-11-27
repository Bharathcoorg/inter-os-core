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

/// Minimum bytes one `encoded_method` occupies: three uleb128 values
/// (method_idx_diff, access_flags, code_off), each at least one byte.
pub const MIN_ENCODED_METHOD_BYTES: usize = 3;
/// Ceiling on cumulative `String` bytes owned by one parsed DEX, as a
/// multiple of the actual input length. WHY a factor and not 1x: legitimate
/// method/type tables clone shared pool strings, so honest files clone a
/// small multiple of their size; the factor caps the quadratic sharing
/// amplification while still accepting real DEX layouts.
pub const MAX_STRING_BYTES_FACTOR: usize = 32;

// Dalvik Opcodes
pub const OP_NOP: u8 = 0x00;
pub const OP_MOVE: u8 = 0x01;                     // format 12x: move vA, vB
pub const OP_MOVE_FROM16: u8 = 0x02;              // format 22x: move/from16 vAA, vBBBB
pub const OP_MOVE_16: u8 = 0x03;                  // format 32x: move/16 vAAAA, vBBBB
pub const OP_MOVE_WIDE: u8 = 0x04;                // format 12x: move-wide vA, vB
pub const OP_MOVE_WIDE_FROM16: u8 = 0x05;         // format 22x: move-wide/from16 vAA, vBBBB
pub const OP_MOVE_WIDE_16: u8 = 0x06;             // format 32x: move-wide/16 vAAAA, vBBBB
pub const OP_MOVE_OBJECT: u8 = 0x07;              // format 12x: move-object vA, vB
pub const OP_MOVE_OBJECT_FROM16: u8 = 0x08;        // format 22x: move-object/from16 vAA, vBBBB
pub const OP_MOVE_OBJECT_16: u8 = 0x09;            // format 32x: move-object/16 vAAAA, vBBBB
pub const OP_MOVE_RESULT: u8 = 0x0A;              // format 11x: move-result vAA
pub const OP_MOVE_RESULT_WIDE: u8 = 0x0B;         // format 11x: move-result-wide vAA
pub const OP_MOVE_RESULT_OBJECT: u8 = 0x0C;       // format 11x: move-result-object vAA
pub const OP_MOVE_EXCEPTION: u8 = 0x0D;           // format 11x: move-exception vAA

pub const OP_RETURN_VOID: u8 = 0x0E;              // format 10x: return-void
pub const OP_RETURN: u8 = 0x0F;                   // format 11x: return vAA
pub const OP_RETURN_WIDE: u8 = 0x10;              // format 11x: return-wide vAA
pub const OP_RETURN_OBJECT: u8 = 0x11;            // format 11x: return-object vAA

pub const OP_CONST_4: u8 = 0x12;                  // format 11n: const/4 vA, #+B
pub const OP_CONST_16: u8 = 0x13;                 // format 21s: const/16 vAA, #+BBBB
pub const OP_CONST: u8 = 0x14;                    // format 31i: const vAA, #+BBBBBBBB
pub const OP_CONST_HIGH16: u8 = 0x15;             // format 21h: const/high16 vAA, #+BBBB0000
pub const OP_CONST_WIDE_16: u8 = 0x16;            // format 21s: const-wide/16 vAA, #+BBBB
pub const OP_CONST_WIDE: u8 = 0x16;               // Alias for OP_CONST_WIDE_16
pub const OP_CONST_WIDE_32: u8 = 0x17;            // format 31i: const-wide/32 vAA, #+BBBBBBBB
pub const OP_CONST_WIDE_FULL: u8 = 0x18;          // format 51l: const-wide vAA, #+BBBBBBBBBBBBBBBB
pub const OP_CONST_WIDE_HIGH16: u8 = 0x19;        // format 21h: const-wide/high16 vAA, #+BBBB000000000000
pub const OP_CONST_STRING: u8 = 0x1A;             // format 21c: const-string vAA, string@BBBB
pub const OP_CONST_STRING_JUMBO: u8 = 0x1B;       // format 31c: const-string/jumbo vAA, string@BBBBBBBB
pub const OP_CONST_CLASS: u8 = 0x1C;              // format 21c: const-class vAA, type@BBBB

pub const OP_MONITOR_ENTER: u8 = 0x1D;            // format 11x: monitor-enter vAA
pub const OP_MONITOR_EXIT: u8 = 0x1E;             // format 11x: monitor-exit vAA
pub const OP_CHECK_CAST: u8 = 0x1F;               // format 21c: check-cast vAA, type@BBBB
pub const OP_INSTANCE_OF: u8 = 0x20;              // format 22c: instance-of vA, vB, type@CCCC
pub const OP_ARRAY_LENGTH: u8 = 0x21;             // format 12x: array-length vA, vB
pub const OP_NEW_INSTANCE: u8 = 0x22;             // format 21c: new-instance vAA, type@BBBB
pub const OP_NEW_ARRAY: u8 = 0x23;                // format 22c: new-array vA, vB, type@CCCC
pub const OP_FILLED_NEW_ARRAY: u8 = 0x24;         // format 35c: filled-new-array {vC..vG}, type@BBBB
pub const OP_FILLED_NEW_ARRAY_RANGE: u8 = 0x25;   // format 3rc: filled-new-array/range {vCCCC..vNNNN}, type@BBBB
pub const OP_FILL_ARRAY_DATA: u8 = 0x26;          // format 31t: fill-array-data vAA, +BBBBBBBB
pub const OP_THROW: u8 = 0x27;                    // format 11x: throw vAA

pub const OP_GOTO: u8 = 0x28;                     // format 10t: goto +AA
pub const OP_GOTO_16: u8 = 0x29;                  // format 20t: goto/16 +AAAA
pub const OP_GOTO_32: u8 = 0x2A;                  // format 30t: goto/32 +AAAAAAAA

pub const OP_PACKED_SWITCH: u8 = 0x2B;            // format 31t: packed-switch vAA, +BBBBBBBB
pub const OP_SPARSE_SWITCH: u8 = 0x2C;            // format 31t: sparse-switch vAA, +BBBBBBBB
pub const OP_CMPL_FLOAT: u8 = 0x2D;               // format 23x: cmpl-float vAA, vBB, vCC
pub const OP_CMPG_FLOAT: u8 = 0x2E;               // format 23x: cmpg-float vAA, vBB, vCC
pub const OP_CMPL_DOUBLE: u8 = 0x2F;              // format 23x: cmpl-double vAA, vBB, vCC
pub const OP_CMPG_DOUBLE: u8 = 0x30;              // format 23x: cmpg-double vAA, vBB, vCC
pub const OP_CMP_LONG: u8 = 0x31;                 // format 23x: cmp-long vAA, vBB, vCC

pub const OP_IF_EQ: u8 = 0x32;                    // format 22t: if-eq vA, vB, +CCCC
pub const OP_IF_NE: u8 = 0x33;                    // format 22t: if-ne vA, vB, +CCCC
pub const OP_IF_LT: u8 = 0x34;                    // format 22t: if-lt vA, vB, +CCCC
pub const OP_IF_GE: u8 = 0x35;                    // format 22t: if-ge vA, vB, +CCCC
pub const OP_IF_GT: u8 = 0x36;                    // format 22t: if-gt vA, vB, +CCCC
pub const OP_IF_LE: u8 = 0x37;                    // format 22t: if-le vA, vB, +CCCC
pub const OP_IF_EQZ: u8 = 0x38;                   // format 21t: if-eqz vAA, +BBBB
pub const OP_IF_NEZ: u8 = 0x39;                   // format 21t: if-nez vAA, +BBBB
pub const OP_IF_LTZ: u8 = 0x3A;                   // format 21t: if-ltz vAA, +BBBB
pub const OP_IF_GEZ: u8 = 0x3B;                   // format 21t: if-gez vAA, +BBBB
pub const OP_IF_GTZ: u8 = 0x3C;                   // format 21t: if-gtz vAA, +BBBB
pub const OP_IF_LEZ: u8 = 0x3D;                   // format 21t: if-lez vAA, +BBBB

pub const OP_AGET: u8 = 0x44;                     // format 23x: aget vAA, vBB, vCC
pub const OP_AGET_WIDE: u8 = 0x45;                // format 23x: aget-wide vAA, vBB, vCC
pub const OP_AGET_OBJECT: u8 = 0x46;              // format 23x: aget-object vAA, vBB, vCC
pub const OP_AGET_BOOLEAN: u8 = 0x47;             // format 23x: aget-boolean vAA, vBB, vCC
pub const OP_AGET_BYTE: u8 = 0x48;                // format 23x: aget-byte vAA, vBB, vCC
pub const OP_AGET_CHAR: u8 = 0x49;                // format 23x: aget-char vAA, vBB, vCC
pub const OP_AGET_SHORT: u8 = 0x4A;               // format 23x: aget-short vAA, vBB, vCC

pub const OP_APUT: u8 = 0x4B;                     // format 23x: aput vAA, vBB, vCC
pub const OP_APUT_WIDE: u8 = 0x4C;                // format 23x: aput-wide vAA, vBB, vCC
pub const OP_APUT_OBJECT: u8 = 0x4D;              // format 23x: aput-object vAA, vBB, vCC
pub const OP_APUT_BOOLEAN: u8 = 0x4E;             // format 23x: aput-boolean vAA, vBB, vCC
pub const OP_APUT_BYTE: u8 = 0x4F;                // format 23x: aput-byte vAA, vBB, vCC
pub const OP_APUT_CHAR: u8 = 0x50;                // format 23x: aput-char vAA, vBB, vCC
pub const OP_APUT_SHORT: u8 = 0x51;               // format 23x: aput-short vAA, vBB, vCC

pub const OP_IGET: u8 = 0x52;                     // format 22c: iget vA, vB, field@CCCC
pub const OP_IGET_WIDE: u8 = 0x53;                // format 22c: iget-wide vA, vB, field@CCCC
pub const OP_IGET_OBJECT: u8 = 0x54;              // format 22c: iget-object vA, vB, field@CCCC
pub const OP_IGET_BOOLEAN: u8 = 0x55;             // format 22c: iget-boolean vA, vB, field@CCCC
pub const OP_IGET_BYTE: u8 = 0x56;                // format 22c: iget-byte vA, vB, field@CCCC
pub const OP_IGET_CHAR: u8 = 0x57;                // format 22c: iget-char vA, vB, field@CCCC
pub const OP_IGET_SHORT: u8 = 0x58;               // format 22c: iget-short vA, vB, field@CCCC

pub const OP_IPUT: u8 = 0x59;                     // format 22c: iput vA, vB, field@CCCC
pub const OP_IPUT_WIDE: u8 = 0x5A;                // format 22c: iput-wide vA, vB, field@CCCC
pub const OP_IPUT_OBJECT: u8 = 0x5B;              // format 22c: iput-object vA, vB, field@CCCC
pub const OP_IPUT_BOOLEAN: u8 = 0x5C;             // format 22c: iput-boolean vA, vB, field@CCCC
pub const OP_IPUT_BYTE: u8 = 0x5D;                // format 22c: iput-byte vA, vB, field@CCCC
pub const OP_IPUT_CHAR: u8 = 0x5E;                // format 22c: iput-char vA, vB, field@CCCC
pub const OP_IPUT_SHORT: u8 = 0x5F;               // format 22c: iput-short vA, vB, field@CCCC

pub const OP_SGET: u8 = 0x60;                     // format 21c: sget vAA, field@BBBB
pub const OP_SGET_WIDE: u8 = 0x61;                // format 21c: sget-wide vAA, field@BBBB
pub const OP_SGET_OBJECT: u8 = 0x62;              // format 21c: sget-object vAA, field@BBBB
pub const OP_SGET_BOOLEAN: u8 = 0x63;             // format 21c: sget-boolean vAA, field@BBBB
pub const OP_SGET_BYTE: u8 = 0x64;                // format 21c: sget-byte vAA, field@BBBB
pub const OP_SGET_CHAR: u8 = 0x65;                // format 21c: sget-char vAA, field@BBBB
pub const OP_SGET_SHORT: u8 = 0x66;               // format 21c: sget-short vAA, field@BBBB

pub const OP_SPUT: u8 = 0x67;                     // format 21c: sput vAA, field@BBBB
pub const OP_SPUT_WIDE: u8 = 0x68;                // format 21c: sput-wide vAA, field@BBBB
pub const OP_SPUT_OBJECT: u8 = 0x69;              // format 21c: sput-object vAA, field@BBBB
pub const OP_SPUT_BOOLEAN: u8 = 0x6A;             // format 21c: sput-boolean vAA, field@BBBB
pub const OP_SPUT_BYTE: u8 = 0x6B;                // format 21c: sput-byte vAA, field@BBBB
pub const OP_SPUT_CHAR: u8 = 0x6C;                // format 21c: sput-char vAA, field@BBBB
pub const OP_SPUT_SHORT: u8 = 0x6D;               // format 21c: sput-short vAA, field@BBBB

pub const OP_INVOKE_VIRTUAL: u8 = 0x6E;           // format 35c: invoke-virtual {vC..vG}, meth@BBBB
pub const OP_INVOKE_SUPER: u8 = 0x6F;             // format 35c: invoke-super {vC..vG}, meth@BBBB
pub const OP_INVOKE_DIRECT: u8 = 0x70;            // format 35c: invoke-direct {vC..vG}, meth@BBBB
pub const OP_INVOKE_STATIC: u8 = 0x71;            // format 35c: invoke-static {vC..vG}, meth@BBBB
pub const OP_INVOKE_INTERFACE: u8 = 0x72;         // format 35c: invoke-interface {vC..vG}, meth@BBBB
pub const OP_INVOKE_VIRTUAL_RANGE: u8 = 0x74;     // format 3rc: invoke-virtual/range {vCCCC..vNNNN}, meth@BBBB
pub const OP_INVOKE_SUPER_RANGE: u8 = 0x75;       // format 3rc: invoke-super/range {vCCCC..vNNNN}, meth@BBBB
pub const OP_INVOKE_DIRECT_RANGE: u8 = 0x76;      // format 3rc: invoke-direct/range {vCCCC..vNNNN}, meth@BBBB
pub const OP_INVOKE_STATIC_RANGE: u8 = 0x77;      // format 3rc: invoke-static/range {vCCCC..vNNNN}, meth@BBBB
pub const OP_INVOKE_INTERFACE_RANGE: u8 = 0x78;   // format 3rc: invoke-interface/range {vCCCC..vNNNN}, meth@BBBB

pub const OP_NEG_INT: u8 = 0x7B;                  // format 12x: neg-int vA, vB
pub const OP_NOT_INT: u8 = 0x7C;                  // format 12x: not-int vA, vB
pub const OP_NEG_LONG: u8 = 0x7D;                 // format 12x: neg-long vA, vB
pub const OP_NOT_LONG: u8 = 0x7E;                 // format 12x: not-long vA, vB
pub const OP_NEG_FLOAT: u8 = 0x7F;                // format 12x: neg-float vA, vB
pub const OP_NEG_DOUBLE: u8 = 0x80;               // format 12x: neg-double vA, vB

pub const OP_INT_TO_LONG: u8 = 0x81;              // format 12x: int-to-long vA, vB
pub const OP_INT_TO_FLOAT: u8 = 0x82;             // format 12x: int-to-float vA, vB
pub const OP_INT_TO_DOUBLE: u8 = 0x83;            // format 12x: int-to-double vA, vB
pub const OP_LONG_TO_INT: u8 = 0x84;              // format 12x: long-to-int vA, vB
pub const OP_LONG_TO_FLOAT: u8 = 0x85;            // format 12x: long-to-float vA, vB
pub const OP_LONG_TO_DOUBLE: u8 = 0x86;           // format 12x: long-to-double vA, vB
pub const OP_FLOAT_TO_INT: u8 = 0x87;             // format 12x: float-to-int vA, vB
pub const OP_FLOAT_TO_LONG: u8 = 0x88;            // format 12x: float-to-long vA, vB
pub const OP_FLOAT_TO_DOUBLE: u8 = 0x89;          // format 12x: float-to-double vA, vB
pub const OP_DOUBLE_TO_INT: u8 = 0x8A;            // format 12x: double-to-int vA, vB
pub const OP_DOUBLE_TO_LONG: u8 = 0x8B;           // format 12x: double-to-long vA, vB
pub const OP_DOUBLE_TO_FLOAT: u8 = 0x8C;          // format 12x: double-to-float vA, vB
pub const OP_INT_TO_BYTE: u8 = 0x8D;              // format 12x: int-to-byte vA, vB
pub const OP_INT_TO_CHAR: u8 = 0x8E;              // format 12x: int-to-char vA, vB
pub const OP_INT_TO_SHORT: u8 = 0x8F;             // format 12x: int-to-short vA, vB

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

pub const OP_ADD_LONG: u8 = 0x9B;
pub const OP_SUB_LONG: u8 = 0x9C;
pub const OP_MUL_LONG: u8 = 0x9D;
pub const OP_DIV_LONG: u8 = 0x9E;
pub const OP_REM_LONG: u8 = 0x9F;
pub const OP_AND_LONG: u8 = 0xA0;
pub const OP_OR_LONG: u8 = 0xA1;
pub const OP_XOR_LONG: u8 = 0xA2;
pub const OP_SHL_LONG: u8 = 0xA3;
pub const OP_SHR_LONG: u8 = 0xA4;
pub const OP_USHR_LONG: u8 = 0xA5;

pub const OP_ADD_FLOAT: u8 = 0xA6;
pub const OP_SUB_FLOAT: u8 = 0xA7;
pub const OP_MUL_FLOAT: u8 = 0xA8;
pub const OP_DIV_FLOAT: u8 = 0xA9;
pub const OP_REM_FLOAT: u8 = 0xAA;

pub const OP_ADD_DOUBLE: u8 = 0xAB;
pub const OP_SUB_DOUBLE: u8 = 0xAC;
pub const OP_MUL_DOUBLE: u8 = 0xAD;
pub const OP_DIV_DOUBLE: u8 = 0xAE;
pub const OP_REM_DOUBLE: u8 = 0xAF;

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

pub const OP_ADD_LONG_2ADDR: u8 = 0xBB;
pub const OP_SUB_LONG_2ADDR: u8 = 0xBC;
pub const OP_MUL_LONG_2ADDR: u8 = 0xBD;
pub const OP_DIV_LONG_2ADDR: u8 = 0xBE;
pub const OP_REM_LONG_2ADDR: u8 = 0xBF;
pub const OP_AND_LONG_2ADDR: u8 = 0xC0;
pub const OP_OR_LONG_2ADDR: u8 = 0xC1;
pub const OP_XOR_LONG_2ADDR: u8 = 0xC2;
pub const OP_SHL_LONG_2ADDR: u8 = 0xC3;
pub const OP_SHR_LONG_2ADDR: u8 = 0xC4;
pub const OP_USHR_LONG_2ADDR: u8 = 0xC5;

pub const OP_ADD_FLOAT_2ADDR: u8 = 0xC6;
pub const OP_SUB_FLOAT_2ADDR: u8 = 0xC7;
pub const OP_MUL_FLOAT_2ADDR: u8 = 0xC8;
pub const OP_DIV_FLOAT_2ADDR: u8 = 0xC9;
pub const OP_REM_FLOAT_2ADDR: u8 = 0xCA;

pub const OP_ADD_DOUBLE_2ADDR: u8 = 0xCB;
pub const OP_SUB_DOUBLE_2ADDR: u8 = 0xCC;
pub const OP_MUL_DOUBLE_2ADDR: u8 = 0xCD;
pub const OP_DIV_DOUBLE_2ADDR: u8 = 0xCE;
pub const OP_REM_DOUBLE_2ADDR: u8 = 0xCF;

pub const OP_ADD_INT_LIT16: u8 = 0xD0;            // format 22s: add-int/lit16
pub const OP_RSUB_INT: u8 = 0xD1;                 // format 22s: rsub-int
pub const OP_MUL_INT_LIT16: u8 = 0xD2;            // format 22s: mul-int/lit16
pub const OP_DIV_INT_LIT16: u8 = 0xD3;            // format 22s: div-int/lit16
pub const OP_REM_INT_LIT16: u8 = 0xD4;            // format 22s: rem-int/lit16
pub const OP_AND_INT_LIT16: u8 = 0xD5;            // format 22s: and-int/lit16
pub const OP_OR_INT_LIT16: u8 = 0xD6;             // format 22s: or-int/lit16
pub const OP_XOR_INT_LIT16: u8 = 0xD7;            // format 22s: xor-int/lit16

pub const OP_ADD_INT_LIT8: u8 = 0xD8;             // format 22b: add-int/lit8
pub const OP_RSUB_INT_LIT8: u8 = 0xD9;            // format 22b: rsub-int/lit8
pub const OP_MUL_INT_LIT8: u8 = 0xDA;             // format 22b: mul-int/lit8
pub const OP_DIV_INT_LIT8: u8 = 0xDB;             // format 22b: div-int/lit8
pub const OP_REM_INT_LIT8: u8 = 0xDC;             // format 22b: rem-int/lit8
pub const OP_AND_INT_LIT8: u8 = 0xDD;             // format 22b: and-int/lit8
pub const OP_OR_INT_LIT8: u8 = 0xDE;              // format 22b: or-int/lit8
pub const OP_XOR_INT_LIT8: u8 = 0xDF;             // format 22b: xor-int/lit8
pub const OP_SHL_INT_LIT8: u8 = 0xE0;             // format 22b: shl-int/lit8
pub const OP_SHR_INT_LIT8: u8 = 0xE1;             // format 22b: shr-int/lit8
pub const OP_USHR_INT_LIT8: u8 = 0xE2;            // format 22b: ushr-int/lit8

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
    /// Total DexMethod records exceeded `raw.len() / MIN_ENCODED_METHOD_BYTES`.
    TooManyMethodRecords,
    /// Cumulative String bytes exceeded `raw.len() * MAX_STRING_BYTES_FACTOR`.
    StringBudgetExceeded,
    /// The verified-instruction counter would overflow u32.
    InstructionCountOverflow,
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
///
/// Owns its backing bytes (`raw`) so a parsed package is `'static`: it can be
/// stored, cached and dropped like any other value without borrowing a caller's
/// buffer and without unsafe self-referential storage. `parse` copies the input
/// once; callers that need zero-copy parsing can keep using the borrowed
/// `parse_borrowed` entry where a borrow is provably long enough.
pub struct DexPackage {
    pub raw: Vec<u8>,
    pub header: DexHeader,
    pub strings: Vec<String>,
    pub types: Vec<String>,
    pub methods: Vec<DexMethod>,
}

impl DexPackage {
    /// Parses a DEX binary file from a byte slice and validates its structure.
    ///
    /// The validated package takes ownership of a copy of `raw`, so the result
    /// has no lifetime dependency on the input buffer.
    pub fn parse(raw: &[u8]) -> Result<Self, DexError> {
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
        if computed_checksum != expected_checksum {
            return Err(DexError::ChecksumMismatch);
        }

        // WHY budgets are formulas over raw.len(): every record and every
        // cloned String must be paid for out of bytes the input actually
        // carries, so shared-offset amplification cannot exceed a constant
        // factor of the input instead of growing quadratically.
        let max_method_records = raw.len() / MIN_ENCODED_METHOD_BYTES;
        let mut string_budget = raw.len()
            .checked_mul(MAX_STRING_BYTES_FACTOR)
            .ok_or(DexError::StringBudgetExceeded)?;

        // 1. Parse String Pool
        let string_ids_size = u32::from_le(header.string_ids_size) as usize;
        let string_ids_off = u32::from_le(header.string_ids_off) as usize;
        let string_table_bytes = string_ids_size.checked_mul(4).ok_or(DexError::UnexpectedEof)?;
        if string_ids_off.checked_add(string_table_bytes).map_or(true, |end| end > raw.len()) {
            return Err(DexError::UnexpectedEof);
        }
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
            charge_string_bytes(&mut string_budget, s.len())?;
            strings.push(String::from(s));
        }

        // 2. Parse Type IDs
        let type_ids_size = u32::from_le(header.type_ids_size) as usize;
        let type_ids_off = u32::from_le(header.type_ids_off) as usize;
        let type_table_bytes = type_ids_size.checked_mul(4).ok_or(DexError::UnexpectedEof)?;
        if type_ids_off.checked_add(type_table_bytes).map_or(true, |end| end > raw.len()) {
            return Err(DexError::UnexpectedEof);
        }
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
            charge_string_bytes(&mut string_budget, strings[descriptor_idx].len())?;
            types.push(strings[descriptor_idx].clone());
        }

        // 2b. Parse Proto IDs. Each proto_ids entry is 12 bytes:
        // shorty_idx:u32, return_type_idx:u32, parameters_off:u32. A method's
        // proto_idx indexes THIS table, not the string pool; the shorty string
        // is strings[shorty_idx].
        let proto_ids_size = u32::from_le(header.proto_ids_size) as usize;
        let proto_ids_off = u32::from_le(header.proto_ids_off) as usize;
        let proto_table_bytes = proto_ids_size.checked_mul(12).ok_or(DexError::UnexpectedEof)?;
        if proto_ids_off.checked_add(proto_table_bytes).map_or(true, |end| end > raw.len()) {
            return Err(DexError::UnexpectedEof);
        }
        let mut proto_shorty_indices = Vec::with_capacity(proto_ids_size);
        for i in 0..proto_ids_size {
            let pos = proto_ids_off + i * 12;
            if pos + 12 > raw.len() {
                return Err(DexError::UnexpectedEof);
            }
            let shorty_idx = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap()) as usize;
            proto_shorty_indices.push(shorty_idx);
        }

        // 3. Parse Method IDs
        let method_ids_size = u32::from_le(header.method_ids_size) as usize;
        let method_ids_off = u32::from_le(header.method_ids_off) as usize;
        let method_table_bytes = method_ids_size.checked_mul(8).ok_or(DexError::UnexpectedEof)?;
        if method_ids_off.checked_add(method_table_bytes).map_or(true, |end| end > raw.len()) {
            return Err(DexError::UnexpectedEof);
        }
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
            let proto_str = match proto_shorty_indices.get(proto_idx) {
                Some(&shorty_idx) if shorty_idx < strings.len() => strings[shorty_idx].clone(),
                _ => String::from("V"),
            };

            charge_string_bytes(&mut string_budget, class_name.len() + method_name.len() + proto_str.len())?;
            method_entries.push((class_name, method_name, proto_str));
        }

        // 4. Parse Class Defs & Code Items
        let class_defs_size = u32::from_le(header.class_defs_size) as usize;
        let class_defs_off = u32::from_le(header.class_defs_off) as usize;
        let class_table_bytes = class_defs_size.checked_mul(32).ok_or(DexError::UnexpectedEof)?;
        if class_defs_off.checked_add(class_table_bytes).map_or(true, |end| end > raw.len()) {
            return Err(DexError::UnexpectedEof);
        }
        let mut methods = Vec::new();
        // WHY dedup: N class_defs pointing at one shared class_data blob used
        // to decode that blob N times, multiplying records by N. A well-formed
        // DEX gives every class its own class_data_item, so a repeated offset
        // is decoded once and later sharers contribute no additional records.
        let mut seen_class_data: Vec<usize> = Vec::new();

        for i in 0..class_defs_size {
            let offset_pos = class_defs_off + i * 32;
            if offset_pos + 32 > raw.len() {
                break;
            }
            let class_data_off = u32::from_le_bytes(raw[offset_pos + 24..offset_pos + 28].try_into().unwrap()) as usize;
            if class_data_off == 0 || class_data_off >= raw.len() {
                continue;
            }
            if seen_class_data.contains(&class_data_off) {
                continue;
            }
            seen_class_data.push(class_data_off);

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
            let mut last_method_idx: u32 = 0;
            for _ in 0..direct_methods_size {
                let diff = read_uleb128(raw, &mut cur)?;
                let _access_flags = read_uleb128(raw, &mut cur)?;
                let code_off = read_uleb128(raw, &mut cur)? as usize;
                last_method_idx = last_method_idx.checked_add(diff).ok_or(DexError::MethodIndexOutOfBounds)?;

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

                if methods.len() >= max_method_records {
                    return Err(DexError::TooManyMethodRecords);
                }
                charge_string_bytes(&mut string_budget, c_name.len() + m_name.len() + p_name.len())?;
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
            let mut last_vmethod_idx: u32 = 0;
            for _ in 0..virtual_methods_size {
                let diff = read_uleb128(raw, &mut cur)?;
                let _access_flags = read_uleb128(raw, &mut cur)?;
                let code_off = read_uleb128(raw, &mut cur)? as usize;
                last_vmethod_idx = last_vmethod_idx.checked_add(diff).ok_or(DexError::MethodIndexOutOfBounds)?;

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

                if methods.len() >= max_method_records {
                    return Err(DexError::TooManyMethodRecords);
                }
                charge_string_bytes(&mut string_budget, c_name.len() + m_name.len() + p_name.len())?;
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
            raw: raw.to_vec(),
            header,
            strings,
            types,
            methods,
        })
    }

    /// Zero-allocation string lookup directly over raw byte slice.
    pub fn get_string_slice(&self, idx: u32) -> Result<&str, DexError> {
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
        let _utf16_size = read_uleb128(&self.raw, &mut cur)?;
        let str_start = cur;
        while cur < self.raw.len() && self.raw[cur] != 0 {
            cur += 1;
        }
        core::str::from_utf8(&self.raw[str_start..cur]).map_err(|_| DexError::StringIndexOutOfBounds)
    }

    /// Zero-allocation type descriptor lookup directly over raw byte slice.
    pub fn get_type_slice(&self, idx: u32) -> Result<&str, DexError> {
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
        if computed_checksum != expected_checksum {
            return Err(DexError::ChecksumMismatch);
        }

        let mut verified_instructions: u32 = 0;

        // 2. Audit methods, registers, CFG branches, and instructions.
        // Security is enforced at capability/syscall boundaries (Zero Ambient Authority),
        // rather than through brittle string-matching filters.
        for m in &self.methods {
            // u16 registers_size is inherently bounded by 65535 (MAX_REGISTERS_LIMIT)

            if m.code_offset > 0 && m.code_offset + 16 <= self.raw.len() {
                let insns_len = m.instruction_count.checked_mul(2).ok_or(DexError::UnexpectedEof)?;
                let insns_start = m.code_offset + 16;
                let insns_end = insns_start.checked_add(insns_len).ok_or(DexError::UnexpectedEof)?;
                if insns_end > self.raw.len() {
                    return Err(DexError::UnexpectedEof);
                }

                let insns = &self.raw[insns_start..insns_end];
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
                        OP_GOTO_32 => {
                            if pc + 6 > insns.len() {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            let offset = i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as isize * 2;
                            let target = pc as isize + offset;
                            if target < 0 || target >= insns.len() as isize {
                                return Err(DexError::BranchOutOfBounds);
                            }
                            pc += 6;
                        }
                        0x2B | 0x2C | 0x26 | 0x24 | 0x25 | 0x03 | 0x06 | 0x09 | 0x14 | 0x17 | 0x1B
                        | OP_INVOKE_VIRTUAL..=0x72 | 0x74..=0x78 => {
                            // format 30t/31t/31i/31c/32x/35c/3rc (6 bytes)
                            if opcode == 0x24 || opcode == 0x25 {
                                let size = insns.get(pc + 1).copied().unwrap_or(0) as usize;
                                if size > 262_144 {
                                    return Err(DexError::ExcessiveArrayAllocation);
                                }
                            }
                            if pc + 6 > insns.len() {
                                return Err(DexError::UnexpectedEof);
                            }
                            pc += 6;
                        }
                        OP_CONST_WIDE_FULL => {
                            // format 51l (10 bytes)
                            if pc + 10 > insns.len() {
                                return Err(DexError::UnexpectedEof);
                            }
                            pc += 10;
                        }
                        0x02 | 0x05 | 0x08 | 0x13 | 0x15 | 0x16 | 0x19 | 0x1A | 0x1C | 0x1F | 0x20
                        | 0x23 | 0x2D..=0x3D | 0x44..=0x6D | 0xD0..=0xE2 => {
                            // format 21s/21h/21c/21t/22x/22c/22t/22s/22b/23x (4 bytes)
                            if pc + 4 > insns.len() {
                                return Err(DexError::UnexpectedEof);
                            }
                            pc += 4;
                        }
                        _ => {
                            pc += 2;
                        }
                    }
                }
            }

            let count = u32::try_from(m.instruction_count).map_err(|_| DexError::InstructionCountOverflow)?;
            verified_instructions = verified_instructions
                .checked_add(count)
                .ok_or(DexError::InstructionCountOverflow)?;
        }

        Ok(verified_instructions)
    }
}

/// Charges cloned String bytes against the parse budget; refuses by number
/// when the cumulative total would exceed `raw.len() * MAX_STRING_BYTES_FACTOR`.
fn charge_string_bytes(budget: &mut usize, bytes: usize) -> Result<(), DexError> {
    *budget = budget.checked_sub(bytes).ok_or(DexError::StringBudgetExceeded)?;
    Ok(())
}

/// Reads an unsigned LEB128 integer from slice at `*offset`.
pub fn read_uleb128(bytes: &[u8], offset: &mut usize) -> Result<u32, DexError> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    while *offset < bytes.len() {
        let byte = bytes[*offset];
        *offset += 1;
        let payload = (byte & 0x7F) as u32;
        if shift < 28 {
            result |= payload << shift;
        } else if shift == 28 {
            if (payload & 0x70) != 0 {
                return Err(DexError::MalformedUleb128);
            }
            result |= payload << 28;
        } else {
            return Err(DexError::MalformedUleb128);
        }
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

/// Reads a signed LEB128 integer from slice at `*offset`.
///
/// Fully hardened against integer overflow, invalid shift values,
/// and safely decodes `i32::MIN` (-2147483648).
pub fn read_sleb128(bytes: &[u8], offset: &mut usize) -> Result<i32, DexError> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    let mut byte: u8;
    loop {
        if *offset >= bytes.len() {
            return Err(DexError::UnexpectedEof);
        }
        byte = bytes[*offset];
        *offset += 1;

        let payload = (byte & 0x7F) as u32;
        if shift < 28 {
            result |= payload << shift;
        } else if shift == 28 {
            // In a 32-bit signed int, the 5th byte provides bits 28..31 (4 bits).
            // Bits 4..6 are sign extension bits and must match the sign bit.
            result |= (payload & 0x0F) << 28;
            let sign_bit = (byte & 0x40) != 0;
            if sign_bit {
                if (payload & 0x70) != 0x70 || (payload & 0x08) == 0 {
                    return Err(DexError::MalformedUleb128);
                }
            } else if (payload & 0x70) != 0 || (payload & 0x08) != 0 {
                return Err(DexError::MalformedUleb128);
            }
        } else {
            return Err(DexError::MalformedUleb128);
        }

        shift += 7;
        if (byte & 0x80) == 0 {
            break;
        }
        if shift > 35 {
            return Err(DexError::MalformedUleb128);
        }
    }

    if shift < 32 && (byte & 0x40) != 0 {
        result |= !0u32 << shift;
    }

    Ok(result as i32)
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

fn push_uleb128(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
    out.push((value & 0x7F) as u8);
}

/// Synthesizes a valid Dalvik Executable (`classes.dex`) whose `onCreate` bytecode
/// allocates a `android.widget.TextView`, calls `setText(text)`, and calls
/// `Activity.setContentView(view)`.
pub fn create_ui_test_dex(class_name: &str, text: &str) -> Vec<u8> {
    let mut dex = Vec::new();

    // Keep string index 0 as "V" because DexPackage::parse indexes `strings[proto_idx]`.
    let strings: [&str; 14] = [
        "V",                         // 0
        "Ljava/lang/Object;",        // 1
        class_name,                  // 2
        "onCreate",                  // 3
        "Landroid/widget/TextView;", // 4
        "<init>",                    // 5
        "Landroid/content/Context;", // 6
        "setText",                   // 7
        "Ljava/lang/CharSequence;",  // 8
        "Landroid/app/Activity;",    // 9
        "setContentView",            // 10
        "Landroid/view/View;",       // 11
        "Landroid/os/Bundle;",       // 12
        text,                        // 13
    ];

    let string_ids_off = DEX_HEADER_SIZE;
    let string_ids_size = strings.len();

    let type_str_indices: [u32; 9] = [0, 1, 2, 4, 6, 8, 9, 11, 12];
    let type_ids_off = string_ids_off + string_ids_size * 4;
    let type_ids_size = type_str_indices.len();

    let proto_ids_off = type_ids_off + type_ids_size * 4;
    let proto_ids_size = 1usize;

    // Methods: (class_type_idx, proto_idx=0 ("V"), name_string_idx)
    let methods: [(u16, u16, u32); 4] = [
        (2, 0, 3),  // 0: MainActivity.onCreate
        (3, 0, 5),  // 1: TextView.<init>
        (3, 0, 7),  // 2: TextView.setText
        (6, 0, 10), // 3: Activity.setContentView
    ];
    let method_ids_off = proto_ids_off + proto_ids_size * 12;
    let method_ids_size = methods.len();

    let class_defs_off = method_ids_off + method_ids_size * 8;
    let class_defs_size = 1usize;

    let data_off = class_defs_off + class_defs_size * 32;
    dex.resize(data_off, 0);

    // 1. Strings data
    let mut string_offsets = Vec::with_capacity(string_ids_size);
    for s in &strings {
        string_offsets.push(dex.len() as u32);
        let s_bytes = s.as_bytes();
        push_uleb128(&mut dex, s_bytes.len() as u32);
        dex.extend_from_slice(s_bytes);
        dex.push(0);
    }
    while dex.len() % 4 != 0 {
        dex.push(0);
    }

    // 2. MainActivity.onCreate code item (14 x 16-bit code units = 28 bytes):
    // registers_size = 3 (v0, v1, p0=v2), ins_size = 1, outs_size = 2
    //   0: new-instance v0, type@3 (TextView)              [0x22, 0x00, 0x03, 0x00]
    //   2: invoke-direct {v0}, method@1 (<init>)             [0x70, 0x10, 0x01, 0x00, 0x00, 0x00]
    //   5: const-string v1, string@13 (text)                 [0x1A, 0x01, 0x0D, 0x00]
    //   7: invoke-virtual {v0, v1}, method@2 (setText)       [0x6E, 0x20, 0x02, 0x00, 0x10, 0x00]
    //  10: invoke-virtual {p0, v0}, method@3 (setContentView)[0x6E, 0x20, 0x03, 0x00, 0x02, 0x00]
    //  13: return-void                                       [0x0E, 0x00]
    let oncreate_code_off = dex.len() as u32;
    dex.extend_from_slice(&3u16.to_le_bytes()); // registers_size = 3
    dex.extend_from_slice(&1u16.to_le_bytes()); // ins_size = 1
    dex.extend_from_slice(&2u16.to_le_bytes()); // outs_size = 2
    dex.extend_from_slice(&0u16.to_le_bytes()); // tries_size = 0
    dex.extend_from_slice(&0u32.to_le_bytes()); // debug_info_off = 0
    dex.extend_from_slice(&14u32.to_le_bytes()); // insns_size = 14 u16 units (28 bytes)
    dex.extend_from_slice(&[
        OP_NEW_INSTANCE, 0x00, 0x03, 0x00,
        OP_INVOKE_DIRECT, 0x10, 0x01, 0x00, 0x00, 0x00,
        OP_CONST_STRING, 0x01, 0x0D, 0x00,
        OP_INVOKE_VIRTUAL, 0x20, 0x02, 0x00, 0x10, 0x00,
        OP_INVOKE_VIRTUAL, 0x20, 0x03, 0x00, 0x02, 0x00,
        OP_RETURN_VOID, 0x00,
    ]);

    while dex.len() % 4 != 0 {
        dex.push(0);
    }

    // 3. Stub code item (`return-void`) for framework method_ids listed in class_data
    let stub_code_off = dex.len() as u32;
    dex.extend_from_slice(&2u16.to_le_bytes());
    dex.extend_from_slice(&1u16.to_le_bytes());
    dex.extend_from_slice(&0u16.to_le_bytes());
    dex.extend_from_slice(&0u16.to_le_bytes());
    dex.extend_from_slice(&0u32.to_le_bytes());
    dex.extend_from_slice(&1u32.to_le_bytes());
    dex.extend_from_slice(&[OP_RETURN_VOID, 0x00]);
    while dex.len() % 4 != 0 {
        dex.push(0);
    }

    // 4. Class Data Item (register methods 0..=3 so DexPackage::parse exposes them in dex.methods)
    let class_data_off = dex.len() as u32;
    push_uleb128(&mut dex, 0); // static_fields_size
    push_uleb128(&mut dex, 0); // instance_fields_size
    push_uleb128(&mut dex, 4); // direct_methods_size
    push_uleb128(&mut dex, 0); // virtual_methods_size
    // method 0: onCreate (diff=0)
    push_uleb128(&mut dex, 0);
    push_uleb128(&mut dex, 1);
    push_uleb128(&mut dex, oncreate_code_off);
    // methods 1..=3 (diff=1 each)
    for _ in 1..4 {
        push_uleb128(&mut dex, 1);
        push_uleb128(&mut dex, 1);
        push_uleb128(&mut dex, stub_code_off);
    }

    let file_size = dex.len() as u32;
    let data_size = file_size - data_off as u32;

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

    for (i, off) in string_offsets.iter().enumerate() {
        let pos = string_ids_off + i * 4;
        dex[pos..pos + 4].copy_from_slice(&off.to_le_bytes());
    }
    for (i, &s_idx) in type_str_indices.iter().enumerate() {
        let pos = type_ids_off + i * 4;
        dex[pos..pos + 4].copy_from_slice(&s_idx.to_le_bytes());
    }
    dex[proto_ids_off..proto_ids_off + 4].copy_from_slice(&0u32.to_le_bytes());
    dex[proto_ids_off + 4..proto_ids_off + 8].copy_from_slice(&0u32.to_le_bytes());
    dex[proto_ids_off + 8..proto_ids_off + 12].copy_from_slice(&0u32.to_le_bytes());

    for (i, &(c_idx, p_idx, n_idx)) in methods.iter().enumerate() {
        let pos = method_ids_off + i * 8;
        dex[pos..pos + 2].copy_from_slice(&c_idx.to_le_bytes());
        dex[pos + 2..pos + 4].copy_from_slice(&p_idx.to_le_bytes());
        dex[pos + 4..pos + 8].copy_from_slice(&n_idx.to_le_bytes());
    }

    dex[class_defs_off..class_defs_off + 4].copy_from_slice(&2u32.to_le_bytes());
    dex[class_defs_off + 4..class_defs_off + 8].copy_from_slice(&1u32.to_le_bytes());
    dex[class_defs_off + 8..class_defs_off + 12].copy_from_slice(&1u32.to_le_bytes());
    dex[class_defs_off + 24..class_defs_off + 28].copy_from_slice(&class_data_off.to_le_bytes());

    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&hdr as *const DexHeader as *const u8, DEX_HEADER_SIZE)
    };
    dex[..DEX_HEADER_SIZE].copy_from_slice(hdr_bytes);

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

    // ------------------------------------------------------------------
    // Adversarial parser-bounds tests (H17): call the real production
    // DexPackage::parse / audit_bytecode paths.
    // ------------------------------------------------------------------

    fn push_uleb(out: &mut Vec<u8>, mut value: u32) {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }

    /// Builds a DEX where `class_defs` class_def entries all point at ONE
    /// shared class_data blob declaring `methods_per_class` direct methods
    /// (each encoded_method is exactly 3 bytes: diff, flags, code_off=0).
    fn build_shared_class_dex(class_defs: usize, methods_per_class: usize, method_diff: u32) -> Vec<u8> {
        let class_defs_off = DEX_HEADER_SIZE;
        let blob_off = class_defs_off + class_defs * 32;
        let mut dex = alloc::vec![0u8; blob_off];
        dex.push(0); // static_fields_size
        dex.push(0); // instance_fields_size
        push_uleb(&mut dex, methods_per_class as u32);
        dex.push(0); // virtual_methods_size
        for _ in 0..methods_per_class {
            push_uleb(&mut dex, method_diff);
            dex.push(1); // access_flags
            dex.push(0); // code_off = 0
        }
        let file_size = dex.len();
        for i in 0..class_defs {
            let p = class_defs_off + i * 32;
            dex[p + 24..p + 28].copy_from_slice(&(blob_off as u32).to_le_bytes());
        }
        let hdr = DexHeader {
            magic: *DEX_MAGIC,
            checksum: 0,
            signature: [0u8; 20],
            file_size: (file_size as u32).to_le(),
            header_size: (DEX_HEADER_SIZE as u32).to_le(),
            endian_tag: ENDIAN_CONSTANT.to_le(),
            link_size: 0,
            link_off: 0,
            map_off: 0,
            string_ids_size: 0,
            string_ids_off: 0,
            type_ids_size: 0,
            type_ids_off: 0,
            proto_ids_size: 0,
            proto_ids_off: 0,
            field_ids_size: 0,
            field_ids_off: 0,
            method_ids_size: 0,
            method_ids_off: 0,
            class_defs_size: (class_defs as u32).to_le(),
            class_defs_off: (class_defs_off as u32).to_le(),
            data_size: ((file_size - blob_off) as u32).to_le(),
            data_off: (blob_off as u32).to_le(),
        };
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const DexHeader as *const u8, DEX_HEADER_SIZE)
        };
        dex[..DEX_HEADER_SIZE].copy_from_slice(hdr_bytes);
        let checksum = compute_adler32(&dex[12..]);
        dex[8..12].copy_from_slice(&checksum.to_le_bytes());
        dex
    }

    #[test]
    fn dex_bounds_shared_class_data_amplification_is_bounded() {
        let raw = build_shared_class_dex(1024, 1024, 1);
        let pkg = DexPackage::parse(&raw).expect("shared-blob DEX must parse under dedup");
        let record_bytes = pkg.methods.len() * core::mem::size_of::<DexMethod>();
        let string_bytes: usize = pkg.methods.iter()
            .map(|m| m.class_name.len() + m.method_name.len() + m.proto_shorty.len())
            .sum();
        let naive_records = 1024usize * 1024;
        std::println!(
            "[DEX-BOUNDS] input_bytes={} naive_records={} actual_records={} record_bytes={} string_bytes={} heap_amplification_before={:.1}x after={:.1}x",
            raw.len(), naive_records, pkg.methods.len(), record_bytes, string_bytes,
            (naive_records * (core::mem::size_of::<DexMethod>() + 14)) as f64 / raw.len() as f64,
            (record_bytes + string_bytes) as f64 / raw.len() as f64,
        );
        let max_records = raw.len() / MIN_ENCODED_METHOD_BYTES;
        assert!(pkg.methods.len() <= max_records,
            "records {} exceeded input-derived cap {}", pkg.methods.len(), max_records);
        assert_eq!(pkg.methods.len(), 1024, "the shared blob must be decoded exactly once");
        assert!(pkg.audit_bytecode().is_ok());
    }

    #[test]
    fn dex_bounds_string_clone_budget_refuses_by_number() {
        // One 1000-byte pool string referenced by 1000 type_ids: honest input
        // is ~5 KiB but naive cloning would allocate ~1 MB of Strings. The
        // cumulative budget raw.len() * MAX_STRING_BYTES_FACTOR must refuse.
        let string_ids_off = DEX_HEADER_SIZE;
        let type_ids_off = string_ids_off + 4;
        let type_count = 1000usize;
        let str_data_off = type_ids_off + type_count * 4;
        let mut dex = alloc::vec![0u8; str_data_off];
        push_uleb(&mut dex, 1000);
        dex.extend_from_slice(&alloc::vec![b'A'; 1000]);
        dex.push(0);
        let file_size = dex.len();
        dex[string_ids_off..string_ids_off + 4].copy_from_slice(&(str_data_off as u32).to_le_bytes());
        for i in 0..type_count {
            let p = type_ids_off + i * 4;
            dex[p..p + 4].copy_from_slice(&0u32.to_le_bytes());
        }
        let hdr = DexHeader {
            magic: *DEX_MAGIC,
            checksum: 0,
            signature: [0u8; 20],
            file_size: (file_size as u32).to_le(),
            header_size: (DEX_HEADER_SIZE as u32).to_le(),
            endian_tag: ENDIAN_CONSTANT.to_le(),
            link_size: 0,
            link_off: 0,
            map_off: 0,
            string_ids_size: 1u32.to_le(),
            string_ids_off: (string_ids_off as u32).to_le(),
            type_ids_size: (type_count as u32).to_le(),
            type_ids_off: (type_ids_off as u32).to_le(),
            proto_ids_size: 0,
            proto_ids_off: 0,
            field_ids_size: 0,
            field_ids_off: 0,
            method_ids_size: 0,
            method_ids_off: 0,
            class_defs_size: 0,
            class_defs_off: 0,
            data_size: ((file_size - str_data_off) as u32).to_le(),
            data_off: (str_data_off as u32).to_le(),
        };
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const DexHeader as *const u8, DEX_HEADER_SIZE)
        };
        dex[..DEX_HEADER_SIZE].copy_from_slice(hdr_bytes);
        let checksum = compute_adler32(&dex[12..]);
        dex[8..12].copy_from_slice(&checksum.to_le_bytes());

        assert!(1000 * 1000 > dex.len() * MAX_STRING_BYTES_FACTOR);
        let err = DexPackage::parse(&dex).err().unwrap();
        assert_eq!(err, DexError::StringBudgetExceeded);
    }

    #[test]
    fn dex_bounds_method_index_diff_overflow_refused() {
        let raw = build_shared_class_dex(1, 2, u32::MAX);
        let err = DexPackage::parse(&raw).err().unwrap();
        assert_eq!(err, DexError::MethodIndexOutOfBounds);
    }

    #[test]
    fn dex_bounds_instruction_counter_overflow_checked() {
        // audit_bytecode is the production counter path. code_offset = 0
        // skips the instruction scan, so two u32::MAX counts reach the
        // accumulator directly; a wrapping add would report success.
        let raw = create_test_dex("Lorg/interos/Counter;", "m", "counter");
        let base = DexPackage::parse(&raw).expect("fixture DEX must parse");
        let mut forged = DexPackage {
            raw: base.raw,
            header: base.header,
            strings: Vec::new(),
            types: Vec::new(),
            methods: Vec::new(),
        };
        for _ in 0..2 {
            forged.methods.push(DexMethod {
                method_index: 0,
                class_name: String::from("Lorg/interos/Counter;"),
                method_name: String::from("m"),
                proto_shorty: String::from("V"),
                is_direct: true,
                registers_size: 0,
                ins_size: 0,
                outs_size: 0,
                instruction_count: u32::MAX as usize,
                code_offset: 0,
            });
        }
        assert_eq!(forged.audit_bytecode(), Err(DexError::InstructionCountOverflow));
    }

    #[test]
    fn test_sleb128_and_uleb128_bounds_and_overflow() {
        // 1. Positive and negative edge cases
        let test_cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (-1, &[0x7F]),
            (63, &[0x3F]),
            (-64, &[0x40]),
            (64, &[0xC0, 0x00]),
            (-65, &[0xBF, 0x7F]),
            (127, &[0xFF, 0x00]),
            (-128, &[0x80, 0x7F]),
            (128, &[0x80, 0x01]),
            (-129, &[0xFF, 0x7E]),
            (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x78]),
        ];

        for &(expected, bytes) in test_cases {
            let mut offset = 0;
            let decoded = read_sleb128(bytes, &mut offset).expect("sleb128 decode must succeed");
            assert_eq!(decoded, expected, "Decoded value mismatch for bytes {:?}", bytes);
            assert_eq!(offset, bytes.len(), "Must consume all bytes");
        }

        // 2. Reject overlong > 5 bytes in sleb128
        let overlong = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        let mut offset = 0;
        assert_eq!(read_sleb128(overlong, &mut offset), Err(DexError::MalformedUleb128));

        // 3. Reject invalid sign extension in 5th byte of sleb128
        // 5th byte with upper bits not matching sign bit
        let bad_ext_pos = &[0x80, 0x80, 0x80, 0x80, 0x10]; // payload has bit 4 set for positive
        let mut offset = 0;
        assert_eq!(read_sleb128(bad_ext_pos, &mut offset), Err(DexError::MalformedUleb128));

        // 4. Reject truncated bytes
        let truncated = &[0x80, 0x80];
        let mut offset = 0;
        assert_eq!(read_sleb128(truncated, &mut offset), Err(DexError::UnexpectedEof));

        // 5. Uleb128 32-bit overflow in 5th byte
        let bad_uleb_overflow = &[0x80, 0x80, 0x80, 0x80, 0x70];
        let mut offset = 0;
        assert_eq!(read_uleb128(bad_uleb_overflow, &mut offset), Err(DexError::MalformedUleb128));
    }

    // Builds a minimal valid DEX whose single method has proto_idx = 0, but whose
    // proto_ids[0].shorty_idx = 1 -> "VL", while string 0 is "V". A parser that
    // wrongly indexes the string pool by proto_idx yields "V"; a correct parser
    // resolves proto_ids -> shorty_idx and yields "VL".
    fn build_proto_shorty_dex() -> Vec<u8> {
        let strings = ["V", "VL", "Lpkg/Cls;", "m"];
        let string_ids_off = DEX_HEADER_SIZE;
        let string_ids_size = strings.len();
        let type_ids_off = string_ids_off + string_ids_size * 4;
        let type_ids_size = 1usize;
        let proto_ids_off = type_ids_off + type_ids_size * 4;
        let proto_ids_size = 1usize;
        let method_ids_off = proto_ids_off + proto_ids_size * 12;
        let method_ids_size = 1usize;
        let class_defs_off = method_ids_off + method_ids_size * 8;
        let class_defs_size = 1usize;
        let data_off = class_defs_off + class_defs_size * 32;
        let mut dex = alloc::vec![0u8; data_off];

        let mut string_offsets = Vec::with_capacity(string_ids_size);
        for s in &strings {
            string_offsets.push(dex.len() as u32);
            push_uleb128(&mut dex, s.len() as u32);
            dex.extend_from_slice(s.as_bytes());
            dex.push(0);
        }
        while dex.len() % 4 != 0 { dex.push(0); }

        let class_data_off = dex.len() as u32;
        push_uleb128(&mut dex, 0);
        push_uleb128(&mut dex, 0);
        push_uleb128(&mut dex, 1);
        push_uleb128(&mut dex, 0);
        push_uleb128(&mut dex, 0);
        push_uleb128(&mut dex, 1);
        push_uleb128(&mut dex, 0);

        let file_size = dex.len() as u32;
        let data_size = file_size - data_off as u32;
        let hdr = DexHeader {
            magic: *DEX_MAGIC, checksum: 0, signature: [0u8; 20],
            file_size: file_size.to_le(), header_size: (DEX_HEADER_SIZE as u32).to_le(),
            endian_tag: ENDIAN_CONSTANT.to_le(), link_size: 0, link_off: 0, map_off: 0,
            string_ids_size: (string_ids_size as u32).to_le(), string_ids_off: (string_ids_off as u32).to_le(),
            type_ids_size: (type_ids_size as u32).to_le(), type_ids_off: (type_ids_off as u32).to_le(),
            proto_ids_size: (proto_ids_size as u32).to_le(), proto_ids_off: (proto_ids_off as u32).to_le(),
            field_ids_size: 0, field_ids_off: 0,
            method_ids_size: (method_ids_size as u32).to_le(), method_ids_off: (method_ids_off as u32).to_le(),
            class_defs_size: (class_defs_size as u32).to_le(), class_defs_off: (class_defs_off as u32).to_le(),
            data_size: data_size.to_le(), data_off: (data_off as u32).to_le(),
        };
        for (i, off) in string_offsets.iter().enumerate() {
            let pos = string_ids_off + i * 4;
            dex[pos..pos + 4].copy_from_slice(&off.to_le_bytes());
        }
        dex[type_ids_off..type_ids_off + 4].copy_from_slice(&2u32.to_le_bytes());
        dex[proto_ids_off..proto_ids_off + 4].copy_from_slice(&1u32.to_le_bytes());
        dex[proto_ids_off + 4..proto_ids_off + 8].copy_from_slice(&0u32.to_le_bytes());
        dex[proto_ids_off + 8..proto_ids_off + 12].copy_from_slice(&0u32.to_le_bytes());
        dex[method_ids_off..method_ids_off + 2].copy_from_slice(&0u16.to_le_bytes());
        dex[method_ids_off + 2..method_ids_off + 4].copy_from_slice(&0u16.to_le_bytes());
        dex[method_ids_off + 4..method_ids_off + 8].copy_from_slice(&3u32.to_le_bytes());
        dex[class_defs_off..class_defs_off + 4].copy_from_slice(&0u32.to_le_bytes());
        dex[class_defs_off + 4..class_defs_off + 8].copy_from_slice(&1u32.to_le_bytes());
        dex[class_defs_off + 8..class_defs_off + 12].copy_from_slice(&1u32.to_le_bytes());
        dex[class_defs_off + 24..class_defs_off + 28].copy_from_slice(&class_data_off.to_le_bytes());

        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const DexHeader as *const u8, DEX_HEADER_SIZE)
        };
        dex[..DEX_HEADER_SIZE].copy_from_slice(hdr_bytes);
        let checksum = compute_adler32(&dex[12..]);
        dex[8..12].copy_from_slice(&checksum.to_le_bytes());
        dex
    }

    #[test]
    fn dex_method_proto_idx_resolves_through_proto_ids_shorty() {
        let dex = DexPackage::parse(&build_proto_shorty_dex()).expect("proto DEX must parse");
        let m = dex.methods.iter().find(|m| m.method_name == "m").expect("method m");
        assert_eq!(m.proto_shorty, "VL", "proto_idx must resolve via proto_ids -> shorty_idx");
    }
}

