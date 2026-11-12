// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Ring 3 ELF Binary Loader
//!
//! Parses 64-bit ELF executables (ET_EXEC), verifies architecture (EM_AARCH64),
//! validates segment structure and power-of-two alignment, allocates physical frames
//! page-by-page, maps pages into an isolated UserAddressSpace with fine-grained W^X permissions,
//! sets up user stack and guard page, and provides atomic rollback on any mapping failure.

extern crate alloc;
use alloc::vec::Vec;
use crate::mmu::{allocate_frame, recycle_frame, UserAddressSpace};

pub const USER_STACK_TOP: u64 = 0x2000_0000;
pub const USER_STACK_SIZE: u64 = 256 * 1024; // 256 KiB stack
pub const USER_CANONICAL_LIMIT: u64 = 0x0000_8000_0000_0000;

pub struct LoadedElf {
    pub entry_point: u64,
    pub stack_top: u64,
    pub address_space: UserAddressSpace,
}

pub fn load_elf(elf_data: &[u8]) -> Result<LoadedElf, &'static str> {
    if elf_data.len() < 64 {
        return Err("ELF image too short");
    }
    if &elf_data[0..4] != b"\x7fELF" {
        return Err("Invalid ELF magic");
    }
    if elf_data[4] != 2 {
        return Err("ELF is not 64-bit");
    }
    if elf_data[5] != 1 {
        return Err("ELF is not little-endian");
    }

    let e_type = u16::from_le_bytes([elf_data[16], elf_data[17]]);
    if e_type != 2 { // ET_EXEC
        return Err("ELF must be ET_EXEC (static executable)");
    }

    let e_machine = u16::from_le_bytes([elf_data[18], elf_data[19]]);
    if e_machine != 0xB7 { // EM_AARCH64
        return Err("ELF machine is not AArch64");
    }

    let e_entry = u64::from_le_bytes(elf_data[24..32].try_into().unwrap());
    if e_entry < 0x1000 || e_entry >= USER_CANONICAL_LIMIT {
        return Err("ELF entry point is outside the canonical user range");
    }

    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes([elf_data[54], elf_data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

    if e_phnum == 0 {
        return Err("ELF has no program headers");
    }
    if e_phentsize != 56 {
        return Err("Invalid program header entry size (expected 56)");
    }

    let stack_top_addr = USER_STACK_TOP;
    let stack_bottom_addr = stack_top_addr.checked_sub(USER_STACK_SIZE).ok_or("Stack address underflow")?;
    let stack_guard_start = stack_bottom_addr.checked_sub(4096).ok_or("Stack guard address underflow")?;
    let stack_guard_end = stack_top_addr;

    let mut aspace = UserAddressSpace::new().ok_or("Failed to create UserAddressSpace")?;
    let mut load_ranges: Vec<(u64, u64, bool, bool)> = Vec::new();
    let mut entry_in_executable_segment = false;

    for i in 0..e_phnum {
        let ph_start = e_phoff + i * e_phentsize;
        if ph_start + e_phentsize > elf_data.len() {
            let _ = aspace.destroy();
            return Err("Program header exceeds ELF file bounds");
        }

        let p_type = u32::from_le_bytes(elf_data[ph_start..ph_start + 4].try_into().unwrap());
        if p_type == 3 {
            let _ = aspace.destroy();
            return Err("PT_INTERP dynamic linkers rejected in static microkernel architecture");
        }
        if p_type != 1 { // PT_LOAD
            continue;
        }

        let p_flags = u32::from_le_bytes(elf_data[ph_start + 4..ph_start + 8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(elf_data[ph_start + 8..ph_start + 16].try_into().unwrap()) as usize;
        let p_vaddr = u64::from_le_bytes(elf_data[ph_start + 16..ph_start + 24].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(elf_data[ph_start + 32..ph_start + 40].try_into().unwrap()) as usize;
        let p_memsz = u64::from_le_bytes(elf_data[ph_start + 40..ph_start + 48].try_into().unwrap()) as usize;
        let p_align = u64::from_le_bytes(elf_data[ph_start + 48..ph_start + 56].try_into().unwrap());

        if p_memsz == 0 {
            continue;
        }

        if p_offset.checked_add(p_filesz).ok_or("Segment file size overflow")? > elf_data.len() {
            let _ = aspace.destroy();
            return Err("Segment file size exceeds ELF binary length");
        }
        if p_filesz > p_memsz {
            let _ = aspace.destroy();
            return Err("Segment file size exceeds memory size");
        }
        if p_vaddr < 0x1000 {
            let _ = aspace.destroy();
            return Err("ELF segment overlaps the reserved null page");
        }
        if p_align != 0 && (p_align & (p_align - 1)) != 0 {
            let _ = aspace.destroy();
            return Err("ELF segment alignment is not a power of two");
        }
        if p_align > 1 && p_vaddr % p_align != (p_offset as u64) % p_align {
            let _ = aspace.destroy();
            return Err("ELF segment virtual/file alignment mismatch");
        }

        let seg_end_vaddr = match p_vaddr.checked_add(p_memsz as u64) {
            Some(end) => end,
            None => {
                let _ = aspace.destroy();
                return Err("Segment memory range overflow");
            }
        };

        if p_vaddr >= seg_end_vaddr || seg_end_vaddr > USER_CANONICAL_LIMIT {
            let _ = aspace.destroy();
            return Err("Invalid or non-canonical user segment range");
        }
        if p_vaddr < stack_guard_end && seg_end_vaddr > stack_guard_start {
            let _ = aspace.destroy();
            return Err("ELF segment overlaps user stack or stack guard region");
        }

        let is_exec = (p_flags & 0x1) != 0;
        let is_write = (p_flags & 0x2) != 0;

        if is_exec && is_write {
            let _ = aspace.destroy();
            return Err("W^X security violation: ELF segment requests both write and execute permissions");
        }

        if load_ranges.iter().any(|(s, e, oe, ow)| p_vaddr < *e && seg_end_vaddr > *s && (*oe != is_exec || *ow != is_write)) {
            let _ = aspace.destroy();
            return Err("Overlapping ELF load segments request conflicting permissions");
        }
        if load_ranges.iter().any(|(s, e, _, _)| p_vaddr < *e && seg_end_vaddr > *s) {
            let _ = aspace.destroy();
            return Err("Overlapping ELF load segments are not supported");
        }
        load_ranges.push((p_vaddr, seg_end_vaddr, is_exec, is_write));

        if is_exec && e_entry >= p_vaddr && e_entry < seg_end_vaddr {
            entry_in_executable_segment = true;
        }

        let start_page = p_vaddr & !0xFFF;
        let end_page = (seg_end_vaddr + 0xFFF) & !0xFFF;

        for page_vaddr in (start_page..end_page).step_by(4096) {
            let frame = match allocate_frame() {
                Some(f) => f,
                None => {
                    let _ = aspace.destroy();
                    return Err("Out of physical memory frames for user segment");
                }
            };

            let page_file_start = p_vaddr.max(page_vaddr);
            let page_file_end = (p_vaddr + p_filesz as u64).min(page_vaddr + 4096);
            if page_file_end > page_file_start {
                let file_src_offset = p_offset + (page_file_start - p_vaddr) as usize;
                let file_copy_len = (page_file_end - page_file_start) as usize;
                let dest_offset = (page_file_start - page_vaddr) as usize;
                unsafe {
                    let dest_ptr = (frame as *mut u8).add(dest_offset);
                    core::ptr::copy_nonoverlapping(elf_data[file_src_offset..].as_ptr(), dest_ptr, file_copy_len);
                }
            }

            if let Err(e) = aspace.map_user_page(page_vaddr, frame, is_write, is_exec) {
                let _ = recycle_frame(frame);
                let _ = aspace.destroy();
                return Err(e);
            }
        }
    }

    if !entry_in_executable_segment {
        let _ = aspace.destroy();
        return Err("ELF entry point is not located within any executable PT_LOAD segment");
    }

    // Allocate user stack (256 KiB + 64 KiB headroom above stack top)
    for page_vaddr in (stack_bottom_addr..stack_top_addr + 65536).step_by(4096) {
        let frame = match allocate_frame() {
            Some(f) => f,
            None => {
                let _ = aspace.destroy();
                return Err("Out of physical memory frames for user stack");
            }
        };
        if let Err(e) = aspace.map_user_page(page_vaddr, frame, true, false) {
            let _ = recycle_frame(frame);
            let _ = aspace.destroy();
            return Err(e);
        }
    }
    // Guard page at stack_guard_start remains unmapped

    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "dsb ish",
            "ic ialluis",
            "dsb ish",
            "isb"
        );
    }

    Ok(LoadedElf {
        entry_point: e_entry,
        stack_top: stack_top_addr,
        address_space: aspace,
    })
}
