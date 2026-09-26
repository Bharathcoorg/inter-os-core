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

const PT_DYNAMIC: u32 = 2;
const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const R_X86_64_RELATIVE: u64 = 8;

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

    // 3. Read program header table offset and count
    crate::serial_println!("[RELOC] Step 3: reading phdr table");
    let e_phoff = core::ptr::read_unaligned((base + 32) as *const u64) as usize;
    let e_phentsize = core::ptr::read_unaligned((base + 54) as *const u16) as usize;
    let e_phnum = core::ptr::read_unaligned((base + 56) as *const u16) as usize;
    crate::serial_println!("[RELOC] phoff={:#X}, phentsize={}, phnum={}", e_phoff, e_phentsize, e_phnum);

    if e_phentsize < core::mem::size_of::<Elf64Phdr>() || e_phnum == 0 || e_phnum > 64 {
        return 0;
    }

    // 4. Locate PT_DYNAMIC segment
    let mut dyn_vaddr: u64 = 0;
    let mut dyn_memsz: u64 = 0;

    for i in 0..e_phnum {
        let phdr_ptr = (base + (e_phoff as u64) + (i * e_phentsize) as u64) as *const Elf64Phdr;
        let p_type = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_type));
        if p_type == PT_DYNAMIC {
            dyn_vaddr = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_vaddr));
            dyn_memsz = core::ptr::read_unaligned(core::ptr::addr_of!((*phdr_ptr).p_memsz));
            break;
        }
    }

    if dyn_vaddr == 0 || dyn_memsz == 0 {
        crate::serial_println!("[RELOC] PT_DYNAMIC not found");
        return 0;
    }
    crate::serial_println!("[RELOC] PT_DYNAMIC found: vaddr={:#X}, memsz={:#X}", dyn_vaddr, dyn_memsz);

    // 5. Parse dynamic table for DT_RELA and DT_RELASZ
    let dyn_ptr = (base + dyn_vaddr) as *const Elf64Dyn;
    let dyn_count = (dyn_memsz as usize) / core::mem::size_of::<Elf64Dyn>();

    let mut rela_vaddr: u64 = 0;
    let mut rela_size: u64 = 0;

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
        }
    }

    if rela_vaddr == 0 || rela_size == 0 {
        crate::serial_println!("[RELOC] DT_RELA or DT_RELASZ not found");
        return 0;
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

    for i in 0..rela_count {
        let rela = rela_ptr.add(i);
        let r_offset = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_offset));
        let r_info = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_info));
        let r_addend = core::ptr::read_unaligned(core::ptr::addr_of!((*rela).r_addend));

        if (r_info & 0xFFFF_FFFF) == R_X86_64_RELATIVE {
            // Guard: ensure target offset resides within writable kernel segments (0x100000..0x300000)
            if r_offset >= 0x100000 && r_offset < 0x300000 {
                let target_addr = (base + r_offset) as *mut u64;
                let relocated_value = base + (r_addend as u64);
                core::ptr::write_unaligned(target_addr, relocated_value);
                applied_count += 1;
            }
        }
    }

    // 8. Restore CR0.WP
    core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));

    applied_count
}
