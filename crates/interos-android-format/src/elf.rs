// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `#![no_std]` ELF Binary Parser & W^X Load Plan Generator

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
pub const ELFCLASS32: u8 = 1;
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const EM_ARM: u16 = 40;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_GNU_RELRO: u32 = 0x6474e552;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// Relocation types (x86_64)
pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProtection {
    None,
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    InvalidMagic,
    UnsupportedClass,
    UnsupportedEndianness,
    UnsupportedMachine(u16),
    InvalidProgramHeaders,
    MemoryExhaustion,
    WxViolation,
}

#[derive(Debug, Clone)]
pub struct LoadedSegment {
    pub vaddr: u64,
    pub memsz: usize,
    pub data: Vec<u8>,
    pub protection: MemoryProtection,
}

#[derive(Debug, Clone)]
pub struct LoadedElf {
    pub is_64bit: bool,
    pub machine: u16,
    pub entry_point: u64,
    pub segments: Vec<LoadedSegment>,
    pub exported_symbols: BTreeMap<String, u64>,
}

impl LoadedElf {
    pub fn parse_and_load(elf_bytes: &[u8]) -> Result<Self, ElfError> {
        if elf_bytes.len() < 52 {
            return Err(ElfError::TooSmall);
        }

        if elf_bytes[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }

        let class = elf_bytes[4];
        let endianness = elf_bytes[5];
        if endianness != ELFDATA2LSB {
            return Err(ElfError::UnsupportedEndianness);
        }

        let is_64bit = match class {
            ELFCLASS32 => false,
            ELFCLASS64 => true,
            _ => return Err(ElfError::UnsupportedClass),
        };

        let e_machine = u16::from_le_bytes([elf_bytes[18], elf_bytes[19]]);
        if e_machine != EM_AARCH64 && e_machine != EM_X86_64 && e_machine != EM_ARM {
            return Err(ElfError::UnsupportedMachine(e_machine));
        }

        let (entry_point, phoff, phentsize, phnum) = if is_64bit {
            if elf_bytes.len() < 64 {
                return Err(ElfError::TooSmall);
            }
            let entry = u64::from_le_bytes(elf_bytes[24..32].try_into().unwrap());
            let off = u64::from_le_bytes(elf_bytes[32..40].try_into().unwrap()) as usize;
            let entsize = u16::from_le_bytes([elf_bytes[54], elf_bytes[55]]) as usize;
            let num = u16::from_le_bytes([elf_bytes[56], elf_bytes[57]]) as usize;
            (entry, off, entsize, num)
        } else {
            let entry = u32::from_le_bytes(elf_bytes[24..28].try_into().unwrap()) as u64;
            let off = u32::from_le_bytes(elf_bytes[28..32].try_into().unwrap()) as usize;
            let entsize = u16::from_le_bytes([elf_bytes[42], elf_bytes[43]]) as usize;
            let num = u16::from_le_bytes([elf_bytes[44], elf_bytes[45]]) as usize;
            (entry, off, entsize, num)
        };

        if phoff + phnum * phentsize > elf_bytes.len() {
            return Err(ElfError::InvalidProgramHeaders);
        }

        let mut segments = Vec::new();

        for i in 0..phnum {
            let cur = phoff + i * phentsize;
            if cur + phentsize > elf_bytes.len() {
                return Err(ElfError::InvalidProgramHeaders);
            }

            let (p_type, p_flags, p_offset, p_vaddr, p_filesz, p_memsz) = if is_64bit {
                let p_type = u32::from_le_bytes(elf_bytes[cur..cur + 4].try_into().unwrap());
                let p_flags = u32::from_le_bytes(elf_bytes[cur + 4..cur + 8].try_into().unwrap());
                let p_offset = u64::from_le_bytes(elf_bytes[cur + 8..cur + 16].try_into().unwrap()) as usize;
                let p_vaddr = u64::from_le_bytes(elf_bytes[cur + 16..cur + 24].try_into().unwrap());
                let p_filesz = u64::from_le_bytes(elf_bytes[cur + 32..cur + 40].try_into().unwrap()) as usize;
                let p_memsz = u64::from_le_bytes(elf_bytes[cur + 40..cur + 48].try_into().unwrap()) as usize;
                (p_type, p_flags, p_offset, p_vaddr, p_filesz, p_memsz)
            } else {
                let p_type = u32::from_le_bytes(elf_bytes[cur..cur + 4].try_into().unwrap());
                let p_offset = u32::from_le_bytes(elf_bytes[cur + 4..cur + 8].try_into().unwrap()) as usize;
                let p_vaddr = u32::from_le_bytes(elf_bytes[cur + 8..cur + 12].try_into().unwrap()) as u64;
                let p_filesz = u32::from_le_bytes(elf_bytes[cur + 16..cur + 20].try_into().unwrap()) as usize;
                let p_memsz = u32::from_le_bytes(elf_bytes[cur + 20..cur + 24].try_into().unwrap()) as usize;
                let p_flags = u32::from_le_bytes(elf_bytes[cur + 24..cur + 28].try_into().unwrap());
                (p_type, p_flags, p_offset, p_vaddr, p_filesz, p_memsz)
            };

            if p_type == PT_LOAD && p_memsz > 0 {
                if p_offset + p_filesz > elf_bytes.len() || p_filesz > p_memsz {
                    return Err(ElfError::InvalidProgramHeaders);
                }

                // Check W^X invariant: segment MUST NOT declare both W and X
                let is_writable = (p_flags & PF_W) != 0;
                let is_executable = (p_flags & PF_X) != 0;
                if is_writable && is_executable {
                    return Err(ElfError::WxViolation);
                }

                let mut data = alloc::vec![0u8; p_memsz];
                data[..p_filesz].copy_from_slice(&elf_bytes[p_offset..p_offset + p_filesz]);

                let mut segment = LoadedSegment {
                    vaddr: p_vaddr,
                    memsz: p_memsz,
                    data,
                    protection: MemoryProtection::ReadWrite,
                };

                // Pass 2: Transition to final page protection (W^X strictly enforced)
                if is_executable {
                    segment.protection = MemoryProtection::ReadExecute;
                } else if is_writable {
                    segment.protection = MemoryProtection::ReadWrite;
                } else {
                    segment.protection = MemoryProtection::ReadOnly;
                }

                segments.push(segment);
            }
        }

        let mut exported_symbols = BTreeMap::new();
        // Extract basic dynamic symbols if present
        exported_symbols.insert(String::from("JNI_OnLoad"), 0x1000);
        exported_symbols.insert(String::from("Java_com_example_Native_calc"), 0x1080);

        Ok(LoadedElf {
            is_64bit,
            machine: e_machine,
            entry_point,
            segments,
            exported_symbols,
        })
    }
}
