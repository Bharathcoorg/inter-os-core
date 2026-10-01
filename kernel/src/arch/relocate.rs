//! Microkernel Early Self-Relocation Engine
//!
//! Applies `R_X86_64_RELATIVE` dynamic relocations when the position-independent
//! kernel ELF is loaded by the bootloader at a higher-half base address
//! (e.g. `0x10000000000`).
//!
//! Conforms strictly to System V AMD64 ELF specifications, resolving Global Offset Table
//! (.got) and data pointers before indirect calls occur.

#[repr(C)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[repr(C)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_W: u32 = 0x2;
const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;
const R_X86_64_RELATIVE: u64 = 8;

/// Upper bound on `e_phnum`, rejecting obviously corrupt headers before we walk the table.
const MAX_PHNUM: usize = 64;
/// Upper bound on `e_phentsize`, rejecting headers whose entries we cannot parse.
const MAX_PHENTSIZE: usize = 512;

/// Applies early self-relocations (R_X86_64_RELATIVE) when loaded by bootloader
/// at a higher-half base address.
/// Returns the number of relocations successfully applied.
#[inline(never)]
pub unsafe fn apply_relocations() -> usize {
    crate::serial_println!("[RELOC] Step 1: determining base");
    // 1. Determine load base address from current RIP
    let rip: u64;
    core::arch::asm!("lea {}, [rip]", out(reg) rip, options(nomem, nostack, preserves_flags));
    let base = rip & 0xFFFF_FFFF_0000_0000;
    crate::serial_println!("[RELOC] RIP={:#X}, Base={:#X}", rip, base);
    if base == 0 {
        return 0;
    }

    // 2. Validate ELF Header magic at base address
    crate::serial_println!("[RELOC] Step 2: reading ELF header at {:#X}", base);
    let elf_header = base as *const u8;
    let b0 = *elf_header;
    let b1 = *elf_header.add(1);
    let b2 = *elf_header.add(2);
    let b3 = *elf_header.add(3);
    crate::serial_println!("[RELOC] Header magic: {:#X} {:#X} {:#X} {:#X}", b0, b1, b2, b3);
    if b0 != 0x7F || b1 != b'E' || b2 != b'L' || b3 != b'F' {
        return 0;
    }

    // 3. Read program header table offset and count.
    crate::serial_println!("[RELOC] Step 3: reading phdr table");
    let e_phoff = core::ptr::read_unaligned((base + 32) as *const u64) as usize;
    let e_phentsize = core::ptr::read_unaligned((base + 54) as *const u16) as usize;
    let e_phnum = core::ptr::read_unaligned((base + 56) as *const u16) as usize;
    crate::serial_println!("[RELOC] phoff={:#X}, phentsize={}, phnum={}", e_phoff, e_phentsize, e_phnum);

    // Bound the phdr table against the ELF header itself. The header lives in the first
    // PT_LOAD, so requiring the whole table to fit in the low 64 KiB (the file-backed
    // prefix of that segment in practice) is conservative and prevents a corrupt
    // e_phoff/e_phnum from turning the scan below into a wild read.
    const ELF_HEADER_SCAN_LIMIT: u64 = 0x1_0000;
    if e_phentsize < core::mem::size_of::<Elf64Phdr>()
        || e_phentsize > MAX_PHENTSIZE
        || e_phnum == 0
        || e_phnum > MAX_PHNUM
    {
        crate::serial_println!("[RELOC] implausible phdr table geometry");
        return 0;
    }
    let phdr_table_bytes = (e_phnum as u64)
        .checked_mul(e_phentsize as u64)
        .and_then(|n| (e_phoff as u64).checked_add(n))
        .filter(|end| *end <= ELF_HEADER_SCAN_LIMIT);
    let Some(phdr_table_end) = phdr_table_bytes else {
        crate::serial_println!("[RELOC] phdr table out of range");
        return 0;
    };
    let _ = phdr_table_end;

    // 4. Locate PT_DYNAMIC and collect the real PT_LOAD bounds.
    //
    // INVARIANT (WHY bounds are derived, not hardcoded): relocation targets must live
    // inside a writable PT_LOAD segment. A hardcoded window silently stops applying
    // relocations once the kernel's writable segment grows past it, leaving unrelocated
    // pointers in .data/.got and producing exactly the null-ish, nondeterministic faults
    // this pass exists to prevent. The window is therefore computed from the program
    // headers of the running image.
    let mut dyn_vaddr: u64 = 0;
    let mut dyn_memsz: u64 = 0;
    // Writable-load bounds, in link-time vaddr space.
    let mut wlo_start: u64 = u64::MAX;
    let mut wlo_end: u64 = 0;
    // Full bounds of every PT_LOAD, used to bounds-check every derived pointer below.
    let mut lo_start: u64 = u64::MAX;
    let mut lo_end: u64 = 0;
    let mut load_count = 0usize;

    for i in 0..e_phnum {
        let phdr_ptr = (base + (e_phoff as u64) + (i * e_phentsize) as u64) as *const Elf64Phdr;
        let p_type = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_type));
        let p_flags = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_flags));
        let p_vaddr = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_vaddr));
        let p_memsz = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_memsz));

        if p_type == PT_LOAD {
            // Reject segments whose memsz would overflow the address space before we use
            // the bound for validation.
            let Some(end) = p_vaddr.checked_add(p_memsz) else {
                crate::serial_println!("[RELOC] PT_LOAD memsz overflow");
                return 0;
            };
            load_count += 1;
            lo_start = lo_start.min(p_vaddr);
            lo_end = lo_end.max(end);
            if p_flags & PF_W != 0 {
                wlo_start = wlo_start.min(p_vaddr);
                wlo_end = wlo_end.max(end);
            }
        }

        if p_type == PT_DYNAMIC && dyn_vaddr == 0 {
            dyn_vaddr = p_vaddr;
            dyn_memsz = p_memsz;
        }
    }

    if load_count == 0 {
        crate::serial_println!("[RELOC] no PT_LOAD segments found");
        return 0;
    }
    crate::serial_println!(
        "[RELOC] PT_LOAD span={:#X}..{:#X}, writable span={:#X}..{:#X}",
        lo_start, lo_end, wlo_start, wlo_end
    );

    if dyn_vaddr == 0 || dyn_memsz == 0 {
        crate::serial_println!("[RELOC] PT_DYNAMIC not found");
        return 0;
    }
    crate::serial_println!("[RELOC] PT_DYNAMIC found: vaddr={:#X}, memsz={:#X}", dyn_vaddr, dyn_memsz);

    // 5. Parse dynamic table for DT_RELA / DT_RELASZ / DT_RELAENT.
    //
    // The dynamic table must be fully inside a PT_LOAD, otherwise we would be reading
    // unmapped memory based on an untrusted offset.
    let dyn_end = dyn_vaddr.saturating_add(dyn_memsz);
    if dyn_vaddr < lo_start || dyn_end > lo_end {
        crate::serial_println!("[RELOC] PT_DYNAMIC outside PT_LOAD bounds");
        return 0;
    }
    let dyn_ptr = (base + dyn_vaddr) as *const Elf64Dyn;
    let dyn_count = (dyn_memsz as usize) / core::mem::size_of::<Elf64Dyn>();

    let mut rela_vaddr: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut rela_ent: u64 = core::mem::size_of::<Elf64Rela>() as u64;

    for i in 0..dyn_count {
        let entry = dyn_ptr.add(i);
        let tag = core::ptr::read_unaligned(core::ptr::addr_of!((*entry).d_tag));
        let val = core::ptr::read_unaligned(core::ptr::addr_of!((*entry).d_val));

        if tag == DT_NULL {
            break;
        } else if tag == DT_RELA {
            rela_vaddr = val;
        } else if tag == DT_RELASZ {
            rela_size = val;
        } else if tag == DT_RELAENT {
            rela_ent = val;
        }
    }

    if rela_vaddr == 0 || rela_size == 0 {
        crate::serial_println!("[RELOC] DT_RELA or DT_RELASZ not found");
        return 0;
    }

    // The relocation table must lie inside a PT_LOAD and its entry size must be exactly
    // one Elf64Rela, otherwise the derived count below would be meaningless.
    if rela_ent != core::mem::size_of::<Elf64Rela>() as u64 {
        crate::serial_println!("[RELOC] unexpected DT_RELAENT {:#X}", rela_ent);
        return 0;
    }
    let rela_end = rela_vaddr.checked_add(rela_size);
    match rela_end {
        Some(end) if rela_vaddr >= lo_start && end <= lo_end => {}
        _ => {
            crate::serial_println!("[RELOC] DT_RELA outside PT_LOAD bounds");
            return 0;
        }
    }
    crate::serial_println!("[RELOC] DT_RELA={:#X}, DT_RELASZ={:#X}", rela_vaddr, rela_size);

    // 6. Disable CR0.WP (Write Protect) during relocation to ensure supervisor writes succeed
    let cr0: u64;
    core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
    core::arch::asm!("mov cr0, {}", in(reg) (cr0 & !(1u64 << 16)), options(nomem, nostack, preserves_flags));

    // 7. Apply all R_X86_64_RELATIVE relocations
    let rela_ptr = (base + rela_vaddr) as *const Elf64Rela;
    let rela_count = (rela_size as usize) / core::mem::size_of::<Elf64Rela>();
    let mut applied_count: usize = 0;
    let mut skipped_count: usize = 0;

    for i in 0..rela_count {
        let rela = rela_ptr.add(i);
        let r_offset = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_offset));
        let r_info = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_info));
        let r_addend = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_addend));

        if (r_info & 0xFFFF_FFFF) == R_X86_64_RELATIVE {
            // Guard against writing outside the image: the 8-byte target slot must lie
            // entirely within the writable PT_LOAD span derived from the program headers.
            // Also check `base + r_offset` cannot overflow the 64-bit address space.
            let slot_fits = r_offset >= wlo_start
                && r_offset.checked_add(core::mem::size_of::<u64>() as u64).map_or(false, |end| end <= wlo_end);
            let target_addr = base.checked_add(r_offset);
            let relocated_value = (r_addend as u64).checked_add(base);

            match (slot_fits, target_addr, relocated_value) {
                (true, Some(target_addr), Some(relocated_value)) => {
                    core::ptr::write_unaligned(target_addr as *mut u64, relocated_value);
                    applied_count += 1;
                }
                _ => {
                    // Never dereference an out-of-bounds slot; count it so the boot log
                    // reveals silent non-application instead of hiding it.
                    skipped_count += 1;
                }
            }
        }
    }

    // 8. Restore CR0.WP
    core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));

    if skipped_count > 0 {
        crate::serial_println!(
            "[RELOC] WARNING: {} relocations outside writable bounds were NOT applied",
            skipped_count
        );
    }

    applied_count
}
