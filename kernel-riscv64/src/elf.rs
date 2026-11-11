// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Ring 3 ELF Binary Loader
//!
//! Parses 64-bit ELF executables (ET_EXEC), verifies architecture (EM_RISCV = 0xF3),
//! dynamically allocates Sv39 user pages, enforces W^X permissions,
//! configures 64 KiB stack with an unmapped guard page, and returns entry point.

use crate::mmu::{PAGE_SIZE, UserAddressSpace, allocate_frame};

pub struct LoadedElf {
    pub entry_point: usize,
    pub stack_top: usize,
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
    if e_machine != 0xF3 { // EM_RISCV
        return Err("ELF machine is not RISC-V");
    }

    let e_entry = u64::from_le_bytes(elf_data[24..32].try_into().unwrap()) as usize;
    let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes([elf_data[54], elf_data[55]]) as usize;
    let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

    if e_phnum == 0 || e_phentsize < 56 {
        return Err("Invalid program header table");
    }

    let stack_top_addr = 0x0080_0000 + 16 * PAGE_SIZE;
    let stack_bottom_addr = 0x0080_0000;
    let stack_guard_start = stack_bottom_addr;
    let stack_guard_end = stack_top_addr;

    let mut aspace = UserAddressSpace::new().ok_or("Failed to create UserAddressSpace")?;
    let mut entry_in_executable_segment = false;

    // Helper closure to load segments; on error aspace.destroy() will be called
    let load_res: Result<(), &'static str> = (|| {
        for i in 0..e_phnum {
            let ph_start = e_phoff + i * e_phentsize;
            if ph_start + e_phentsize > elf_data.len() {
                return Err("Program header exceeds ELF file bounds");
            }

            let p_type = u32::from_le_bytes(elf_data[ph_start..ph_start + 4].try_into().unwrap());
            if p_type == 3 { // PT_INTERP
                return Err("Dynamic binaries with PT_INTERP not supported");
            }
            if p_type != 1 { // Only PT_LOAD
                continue;
            }

            let p_flags = u32::from_le_bytes(elf_data[ph_start + 4..ph_start + 8].try_into().unwrap());
            let p_offset = u64::from_le_bytes(elf_data[ph_start + 8..ph_start + 16].try_into().unwrap()) as usize;
            let p_vaddr = u64::from_le_bytes(elf_data[ph_start + 16..ph_start + 24].try_into().unwrap()) as usize;
            let p_filesz = u64::from_le_bytes(elf_data[ph_start + 32..ph_start + 40].try_into().unwrap()) as usize;
            let p_memsz = u64::from_le_bytes(elf_data[ph_start + 40..ph_start + 48].try_into().unwrap()) as usize;

            if p_offset + p_filesz > elf_data.len() {
                return Err("Segment file size exceeds ELF file bounds");
            }
            if p_filesz > p_memsz {
                return Err("Segment file size exceeds memory size");
            }

            let seg_end_vaddr = p_vaddr.checked_add(p_memsz).ok_or("Segment vaddr overflow")?;
            if p_vaddr < stack_guard_end && seg_end_vaddr > stack_guard_start {
                return Err("Segment collides with user stack region");
            }

            let is_exec = (p_flags & 1) != 0;
            let is_write = (p_flags & 2) != 0;

            if is_write && is_exec {
                return Err("Segment violates W^X: simultaneously writable and executable");
            }

            if is_exec && e_entry >= p_vaddr && e_entry < seg_end_vaddr {
                entry_in_executable_segment = true;
            }

            let page_start = p_vaddr & !(PAGE_SIZE - 1);
            let page_end = (seg_end_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

            let mut cur_page = page_start;
            while cur_page < page_end {
                let frame = allocate_frame().ok_or("Out of physical memory allocating ELF page")?;
                aspace.map_user_page(cur_page, frame, is_write, is_exec)?;

                let page_file_start = p_vaddr.max(cur_page);
                let page_file_end = (p_vaddr + p_filesz).min(cur_page + PAGE_SIZE);

                if page_file_end > page_file_start {
                    let file_src_offset = p_offset + (page_file_start - p_vaddr);
                    let file_copy_len = page_file_end - page_file_start;
                    let dest_offset = page_file_start - cur_page;

                    unsafe {
                        let dst = (frame as *mut u8).add(dest_offset);
                        core::ptr::copy_nonoverlapping(
                            elf_data[file_src_offset..].as_ptr(),
                            dst,
                            file_copy_len,
                        );
                    }
                }

                cur_page += PAGE_SIZE;
            }
        }

        // Allocate user stack (16 pages = 64 KiB)
        // Guard page at stack_bottom_addr remains unmapped
        for p in 1..16 {
            let stack_page_vaddr = stack_bottom_addr + p * PAGE_SIZE;
            let frame = allocate_frame().ok_or("Out of physical memory allocating user stack")?;
            aspace.map_user_page(stack_page_vaddr, frame, true, false)?;
        }

        if !entry_in_executable_segment {
            return Err("ELF entry point is not inside an executable segment");
        }

        Ok(())
    })();

    if let Err(e) = load_res {
        aspace.destroy();
        return Err(e);
    }

    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence.i");
    }

    Ok(LoadedElf {
        entry_point: e_entry,
        stack_top: stack_top_addr,
        address_space: aspace,
    })
}
