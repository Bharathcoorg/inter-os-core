// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Refusal of destructive writes to the device this kernel booted from.
//!
//! WHY this module exists: `InterFs::format_region` and the persistent journal
//! both write into the region `filesystem::BLOCK_FS_BASE_LBA` describes. On a
//! single-disk image that region is *inside* the bootable area, because the
//! bootloader's disk layout puts this very kernel image there. Formatting it
//! destroys the FAT filesystem that holds `kernel-x86_64`, so the second boot of
//! the same image faults before userspace starts.
//!
//! WHY nothing here is a hardcoded LBA. The guard needs two facts:
//!
//! 1. *What this kernel image is* Ã¢â‚¬â€ derived by walking the program headers of
//!    the running image, exactly the way `arch::relocate::apply_relocations`
//!    derives its own bounds. `__ehdr_start` is the linker-provided address of
//!    our own ELF header, so the load base is a property of the image rather
//!    than a constant in the source.
//! 2. *Which sectors of the candidate device the firmware could have loaded it
//!    from* Ã¢â‚¬â€ derived from the candidate device's own MBR partition table. The
//!    bootloader's disk layout is authored by that table, so the bootable region
//!    is `0 .. end_of_last_partition`, and it moves with the layout.
//!
//! WHY the two facts cannot be compared directly: one is a load address, the
//! other is a sector address, and nothing in a booting kernel maps between them
//! Ã¢â‚¬â€ the bootloader copies the kernel out of a filesystem into whatever physical
//! address is free. So the PT_LOAD extent is used as the *witness* that the
//! device's advertised boot area must be able to hold, and the advertised boot
//! area is the protected range. Both sides are read at runtime; the only
//! fallback is to refuse.
//!
//! WHY refusing is correct rather than merely cautious: the alternative is
//! silently overwriting the code that is currently executing. A user whose
//! writes go nowhere is told about it on the console; a user whose bootloader is
//! destroyed finds out on the next boot.

use alloc::format;
use alloc::string::String;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};

/// `e_ident` magic of a 64-bit ELF image.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const E_TYPE: usize = 16;
const E_PHOFF: usize = 32;
const E_PHENTSIZE: usize = 54;
const E_PHNUM: usize = 56;
const PH_TYPE: usize = 0;
const PH_OFFSET: usize = 8;
const PH_VADDR: usize = 16;
const PH_PADDR: usize = 24;
const PH_FILESZ: usize = 32;
const PH_MEMSZ: usize = 40;
/// Upper bound on `e_phnum`, so corrupt geometry cannot turn the walk below into
/// a wild read. Mirrors `arch::relocate`.
const MAX_PHNUM: u16 = 64;
const MIN_PHENTSIZE: u16 = 56;
const MAX_PHENTSIZE: u16 = 512;
/// Bound on where the program header table may live, relative to the ELF header.
///
/// WHY: the header is inside the first PT_LOAD, whose file-backed prefix is the
/// first few KiB in practice. Bounding the table here keeps a corrupt `e_phoff`
/// or `e_phnum` from sending the parse into unrelated memory.
const MAX_PHD_TABLE_SPAN: u64 = 0x1_0000;

/// Offset of the MBR partition table inside the boot sector.
const MBR_PARTITION_TABLE: usize = 446;
const MBR_PARTITION_ENTRY: usize = 16;
const MBR_ENTRIES: usize = 4;
const MBR_ENTRY_STATUS: usize = 0;
const MBR_ENTRY_TYPE: usize = 4;
const MBR_ENTRY_START_LBA: usize = 8;
const MBR_ENTRY_SECTORS: usize = 12;
const MBR_BOOT_ACTIVE: u8 = 0x80;

/// A half-open sector interval, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorRange {
    pub start: u32,
    pub end: u32,
}

impl SectorRange {
    /// A range of `len` sectors starting at `start`, or `None` if it would wrap.
    pub fn new(start: u32, len: u32) -> Option<Self> {
        Some(Self { start, end: start.checked_add(len)? })
    }

    /// Half-open intersection test. Touching endpoints do not overlap: a range
    /// ending exactly where the protected region begins is entirely outside it.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn sectors(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// The extent of the image this kernel is executing from.
///
/// `vaddr_*` is in the bootloader's virtual mapping, `paddr_*` in physical
/// memory, and `file_bytes` is the on-disk size the ELF itself declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageExtent {
    /// Address of this image's ELF header at run time.
    pub hdr_vaddr: u64,
    pub vaddr_start: u64,
    pub vaddr_end: u64,
    pub paddr_start: u64,
    pub paddr_end: u64,
    pub file_bytes: u64,
}

impl ImageExtent {
    pub fn mem_bytes(&self) -> u64 {
        self.vaddr_end.saturating_sub(self.vaddr_start)
    }

    /// On-disk size of the image rounded up to whole sectors.
    pub fn file_sectors(&self) -> u64 {
        let s = SECTOR_SIZE as u64;
        (self.file_bytes + s - 1) / s
    }

    /// Whether the link-time physical and virtual addresses agree.
    ///
    /// WHY this is reported rather than assumed: the guard's sector arithmetic
    /// only needs the image's *size*, so a non-identity mapping is not fatal. It
    /// is surfaced so the boot log says which of the two ranges it derived the
    /// numbers from instead of implying a correspondence that may not exist.
    pub fn paddr_is_identity(&self) -> bool {
        self.paddr_start == self.vaddr_start
    }
}

/// Reads the program headers of the ELF image whose header is at `hdr_vaddr`.
///
/// `read` abstracts the address space so the same parser serves the bare-metal
/// walk and the unit tests, which hand it a buffer.
pub fn parse_extent_at<F>(hdr_vaddr: u64, read: &F) -> Option<ImageExtent>
where
    F: Fn(u64) -> Option<u8>,
{
    let byte = |off: u64| read(hdr_vaddr.checked_add(off)?);
    let word16 = |off: u64| -> Option<u16> {
        Some(u16::from_le_bytes([byte(off)?, byte(off.checked_add(1)?)?]))
    };
    let word32 = |off: u64| -> Option<u32> {
        Some(u32::from_le_bytes([
            byte(off)?,
            byte(off.checked_add(1)?)?,
            byte(off.checked_add(2)?)?,
            byte(off.checked_add(3)?)?,
        ]))
    };
    let word64 = |off: u64| -> Option<u64> {
        let mut raw = [0u8; 8];
        for (i, slot) in raw.iter_mut().enumerate() {
            *slot = byte(off.checked_add(i as u64)?)?;
        }
        Some(u64::from_le_bytes(raw))
    };

    for (i, expected) in ELF_MAGIC.iter().enumerate() {
        if byte(i as u64)? != *expected {
            return None;
        }
    }
    if byte(EI_CLASS as u64)? != ELFCLASS64 || byte(EI_DATA as u64)? != ELFDATA2LSB {
        return None;
    }
    // ET_DYN (position independent) is what the bootloader loads; ET_EXEC is
    // accepted so a fixed-address build is guarded identically rather than
    // silently skipped.
    let e_type = word16(E_TYPE as u64)?;
    if e_type != 2 && e_type != 3 {
        return None;
    }
    if word16(18)? != EM_X86_64 {
        return None;
    }

    let e_phoff = word64(E_PHOFF as u64)?;
    let e_phentsize = word16(E_PHENTSIZE as u64)? as u64;
    let e_phnum = word16(E_PHNUM as u64)?;
    if e_phentsize < MIN_PHENTSIZE as u64
        || e_phentsize > MAX_PHENTSIZE as u64
        || e_phnum == 0
        || e_phnum > MAX_PHNUM
    {
        return None;
    }
    let table_end = e_phoff.checked_add(e_phentsize.checked_mul(e_phnum as u64)?)?;
    if table_end > MAX_PHD_TABLE_SPAN {
        return None;
    }

    let mut vaddr_start = u64::MAX;
    let mut vaddr_end = 0u64;
    let mut paddr_start = u64::MAX;
    let mut paddr_end = 0u64;
    let mut file_bytes = 0u64;
    let mut loads = 0u32;

    for i in 0..e_phnum as u64 {
        let base = e_phoff.checked_add(i.checked_mul(e_phentsize)?)?;
        if word32(base.checked_add(PH_TYPE as u64)?)? != PT_LOAD {
            continue;
        }
        let p_offset = word64(base.checked_add(PH_OFFSET as u64)?)?;
        let p_vaddr = word64(base.checked_add(PH_VADDR as u64)?)?;
        let p_paddr = word64(base.checked_add(PH_PADDR as u64)?)?;
        let p_filesz = word64(base.checked_add(PH_FILESZ as u64)?)?;
        let p_memsz = word64(base.checked_add(PH_MEMSZ as u64)?)?;
        let v_end = p_vaddr.checked_add(p_memsz)?;
        let p_end = p_paddr.checked_add(p_memsz)?;
        if p_end < p_paddr || v_end < p_vaddr {
            return None;
        }
        loads += 1;
        vaddr_start = vaddr_start.min(p_vaddr);
        vaddr_end = vaddr_end.max(v_end);
        paddr_start = paddr_start.min(p_paddr);
        paddr_end = paddr_end.max(p_end);
        file_bytes = file_bytes.max(p_offset.checked_add(p_filesz)?);
    }

    if loads == 0 || file_bytes == 0 {
        return None;
    }
    Some(ImageExtent {
        hdr_vaddr,
        vaddr_start: hdr_vaddr.checked_add(vaddr_start)?,
        vaddr_end: hdr_vaddr.checked_add(vaddr_end)?,
        paddr_start: hdr_vaddr.checked_add(paddr_start)?,
        paddr_end: hdr_vaddr.checked_add(paddr_end)?,
        file_bytes,
    })
}

/// The extent of the image this kernel is executing from, derived from its own
/// ELF program headers.
///
/// WHY `__ehdr_start` rather than a backwards memory scan for `\x7FELF`: the
/// linker places that symbol at the ELF header of the image being linked, so it
/// is the load base by construction. A scan would have to dereference addresses
/// below our own image to find it, which in a kernel is a page fault waiting to
/// happen, and it would be wrong the moment the image stopped being ELF.
#[cfg(not(test))]
pub fn current_image() -> Option<ImageExtent> {
    extern "C" {
        static __ehdr_start: u8;
    }
    // WHY `unsafe` on the symbol reference only: taking the address of an extern
    // static is unsafe because the linker, not the compiler, guarantees what is
    // there. Nothing is dereferenced yet; `parse_extent_at` bounds every read.
    let base = unsafe { &__ehdr_start as *const u8 as u64 };
    // WHY unchecked reads: `base` is the linker symbol for this image's own ELF
    // header, and every offset below is bounded by `MAX_PHD_TABLE_SPAN`, so the
    // walk stays inside the header and its program header table.
    parse_extent_at(base, &|addr| Some(unsafe { *(addr as *const u8) }))
}

/// WHY the `cfg(test)` stand-in is a real parsed ELF rather than a literal: the
/// end-to-end tests below have to decide whether a device with a boot sector is
/// refused and a device without one is not, and that decision consumes the image
/// extent. A hardcoded struct would let the two disagree about what the running
/// image looks like. The stand-in is built and parsed by the same code the
/// kernel runs, from program headers shaped like a packaged kernel's.
#[cfg(test)]
pub fn current_image() -> Option<ImageExtent> {
    /// Header, text and data segments of the packaged boot image's kernel ELF,
    /// read out of `target/inter-os-bios.img` at LBA 1551.
    const PHDRS: [(u32, u64, u64, u64, u64, u64); 3] = [
        (PT_LOAD, 0x0, 0x0, 0x0, 0x11F134, 0x11F134),
        (PT_LOAD, 0x11F140, 0x120140, 0x120140, 0xD6BCB, 0xD6BCB),
        (PT_LOAD, 0x1F5D10, 0x1F7D10, 0x1F7D10, 0x3B370, 0x6CA7A),
    ];
    const BASE: u64 = 0x40_0000;
    const PHOFF: u64 = 64;

    let mut image = alloc::vec![0u8; (PHOFF + 56 * 3) as usize];
    image[0..4].copy_from_slice(&ELF_MAGIC);
    image[EI_CLASS] = ELFCLASS64;
    image[EI_DATA] = ELFDATA2LSB;
    image[E_TYPE..E_TYPE + 2].copy_from_slice(&3u16.to_le_bytes());
    image[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
    image[E_PHOFF..E_PHOFF + 8].copy_from_slice(&PHOFF.to_le_bytes());
    image[E_PHENTSIZE..E_PHENTSIZE + 2].copy_from_slice(&56u16.to_le_bytes());
    image[E_PHNUM..E_PHNUM + 2].copy_from_slice(&(PHDRS.len() as u16).to_le_bytes());
    for (i, (p_type, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz)) in PHDRS.iter().enumerate() {
        let o = (PHOFF + i as u64 * 56) as usize;
        image[o + PH_TYPE..o + PH_TYPE + 4].copy_from_slice(&p_type.to_le_bytes());
        image[o + PH_OFFSET..o + PH_OFFSET + 8].copy_from_slice(&p_offset.to_le_bytes());
        image[o + PH_VADDR..o + PH_VADDR + 8].copy_from_slice(&p_vaddr.to_le_bytes());
        image[o + PH_PADDR..o + PH_PADDR + 8].copy_from_slice(&p_paddr.to_le_bytes());
        image[o + PH_FILESZ..o + PH_FILESZ + 8].copy_from_slice(&p_filesz.to_le_bytes());
        image[o + PH_MEMSZ..o + PH_MEMSZ + 8].copy_from_slice(&p_memsz.to_le_bytes());
    }
    parse_extent_at(BASE, &|addr| {
        // WHY the subtraction: `parse_extent_at` reports addresses relative to
        // the image as it is mapped, while the byte image is indexed from zero.
        addr.checked_sub(BASE).and_then(|i| image.get(i as usize).copied())
    })
}

/// What a candidate device says about itself as boot media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootArea {
    /// LBA 0 ends with the 0x55AA signature.
    pub signature: bool,
    /// Partition entries marked active/bootable.
    pub active_entries: u32,
    /// Lowest starting LBA of any non-empty entry, or `u32::MAX` if there is none.
    pub first_lba: u32,
    /// Exclusive end of the highest non-empty entry.
    pub end_lba: u32,
}

impl BootArea {
    /// The sectors the firmware can load an operating system from.
    pub fn range(&self) -> SectorRange {
        SectorRange { start: 0, end: self.end_lba }
    }

    /// Whether this device could have been booted from at all.
    pub fn is_boot_media(&self) -> bool {
        self.signature
    }
}

/// Derives the bootable region of a candidate device from its own boot sector.
///
/// WHY the partition table and not a fixed sector: the bootloader's disk writer
/// places the second stage, the FAT boot partition and this kernel inside the
/// partitions it records here. Repacking the image moves those partitions and
/// this bound moves with them.
pub fn parse_boot_area(sector: &[u8; SECTOR_SIZE]) -> BootArea {
    let mut area = BootArea { signature: false, active_entries: 0, first_lba: u32::MAX, end_lba: 0 };
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return area;
    }
    area.signature = true;
    for i in 0..MBR_ENTRIES {
        let off = MBR_PARTITION_TABLE + i * MBR_PARTITION_ENTRY;
        let status = sector[off + MBR_ENTRY_STATUS];
        if status & MBR_BOOT_ACTIVE != 0 {
            area.active_entries += 1;
        }
        let sys = sector[off + MBR_ENTRY_TYPE];
        if sys == 0 {
            continue;
        }
        let start = u32::from_le_bytes([
            sector[off + MBR_ENTRY_START_LBA],
            sector[off + MBR_ENTRY_START_LBA + 1],
            sector[off + MBR_ENTRY_START_LBA + 2],
            sector[off + MBR_ENTRY_START_LBA + 3],
        ]);
        let sectors = u32::from_le_bytes([
            sector[off + MBR_ENTRY_SECTORS],
            sector[off + MBR_ENTRY_SECTORS + 1],
            sector[off + MBR_ENTRY_SECTORS + 2],
            sector[off + MBR_ENTRY_SECTORS + 3],
        ]);
        if sectors == 0 {
            continue;
        }
        area.first_lba = area.first_lba.min(start);
        if let Some(end) = start.checked_add(sectors) {
            area.end_lba = area.end_lba.max(end);
        }
    }
    area
}

/// Why a write range was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The running image's extent could not be established, so nothing can
    /// prove the target range is disjoint from it.
    SelfImageUnknown,
    /// LBA 0 could not be read, so the device's boot area is unknown.
    BootSectorUnreadable,
    /// The running image is larger than the boot area the device advertises, so
    /// the model of where the firmware loads this kernel from is wrong.
    LayoutUnverifiable { image_sectors: u64, boot_area_sectors: u32 },
    /// The target range intersects the region the firmware loads this kernel from.
    OverlapsBootMedia { boot_area_end: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteVerdict {
    Permitted,
    Refused(Refusal),
}

/// Decides whether `target` may be written on a device described by `boot`.
///
/// Every branch that cannot establish safety returns `Refused`. There is no
/// branch that assumes a range is safe because it "looks far enough away": the
/// only inputs are the running image's own declared size and the device's own
/// declared boot area.
pub fn evaluate_writes(
    image: Option<&ImageExtent>,
    boot: &BootArea,
    device_sectors: u32,
    target: SectorRange,
) -> WriteVerdict {
    let Some(image) = image else {
        return WriteVerdict::Refused(Refusal::SelfImageUnknown);
    };
    // No 0x55AA boot signature means no BIOS boot chain could have started from
    // this device, so it cannot be holding the code now executing. This is the
    // one case that lets a separate data disk be used, and it is a property of
    // the device rather than an assumption about which one was plugged in.
    if !boot.is_boot_media() {
        return WriteVerdict::Permitted;
    }
    // Clamp to the device we actually hold: a partition table claiming more
    // sectors than the drive reports must not produce an unbounded protected
    // range, and must not be read as licence to write either.
    let boot_area_end = core::cmp::min(boot.end_lba, device_sectors);
    if boot_area_end == 0 {
        // A signature but no usable partition: we cannot tell what is loaded
        // from here, so we cannot prove the target is clear of it.
        return WriteVerdict::Refused(Refusal::LayoutUnverifiable {
            image_sectors: image.file_sectors(),
            boot_area_sectors: 0,
        });
    }
    if image.file_sectors() > boot_area_end as u64 {
        // WHY the self-consistency check, knowing it can refuse a usable disk: a
        // device that presents a boot sector but advertises a boot area too
        // small to hold the image now executing cannot be where we were loaded
        // from, so either our parse of it is wrong or it is not the device we
        // think it is. Either way the target range cannot be shown to be clear
        // of the running image.
        //
        // The cost is real and accepted: a data disk that happens to carry an
        // MBR whose boot area is smaller than this kernel will be refused too.
        // That is the deliberate direction of the trade — the alternative is a
        // guard that silently stops guarding when the layout is not the one it
        // was written against — and the refusal states both numbers so the user
        // can see what to change.
        return WriteVerdict::Refused(Refusal::LayoutUnverifiable {
            image_sectors: image.file_sectors(),
            boot_area_sectors: boot_area_end,
        });
    }
    if target.overlaps(&SectorRange { start: 0, end: boot_area_end }) {
        return WriteVerdict::Refused(Refusal::OverlapsBootMedia { boot_area_end });
    }
    WriteVerdict::Permitted
}

/// Reads the candidate device's own boot sector and evaluates `target`.
///
/// This is the production entry point: the image extent comes from the running
/// image's program headers and the boot area from the device being written.
///
/// WHY generic over `?Sized` rather than taking `&mut dyn BlockDevice`: the
/// filesystem backend is generic over `D: BlockDevice + ?Sized` and holds
/// `&mut D`, so a hard `dyn` parameter here would not accept it.
pub fn evaluate_device_writes<D: BlockDevice + ?Sized>(device: &mut D, target: SectorRange) -> WriteVerdict {
    let total = device.sector_count();
    let mut sector = [0u8; SECTOR_SIZE];
    if device.read_sector(0, &mut sector).is_err() {
        return WriteVerdict::Refused(Refusal::BootSectorUnreadable);
    }
    evaluate_writes(current_image().as_ref(), &parse_boot_area(&sector), total, target)
}

/// The complete console text for a refused write, built as data so the exact
/// wording the user sees can be asserted in a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusalNotice {
    pub lines: [String; 4],
}

impl RefusalNotice {
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, index: usize) -> &str {
        &self.lines[index]
    }

    /// Every line joined by newlines, for assertions and for the console.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for line in self.lines.iter() {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// Builds the diagnostic printed when a write is refused.
///
/// `scope` names what was declined ("InterFS format", "persistent journal
/// commit"), so the console says which operation was skipped rather than
/// leaving the user to infer it from a missing "[OK]".
pub fn refuse_notice(
    scope: &str,
    model: &str,
    target: SectorRange,
    boot: Option<&BootArea>,
    refusal: Refusal,
) -> RefusalNotice {
    let image_line = match current_image() {
        Some(image) => format!(
            "[STORAGE]   this kernel image: {} sectors on disk, loaded at {:#X}..{:#X}",
            image.file_sectors(),
            image.vaddr_start,
            image.vaddr_end
        ),
        None => "[STORAGE]   this kernel image: extent could not be derived from its own ELF headers".into(),
    };
    let (reason, detail): (&str, String) = match refusal {
        Refusal::SelfImageUnknown => (
            "the extent of the running kernel image could not be derived, so the target range cannot be proven disjoint from it",
            "[STORAGE]   refusing to write rather than assume the range is clear".into(),
        ),
        Refusal::BootSectorUnreadable => (
            "LBA 0 could not be read, so this device's boot area is unknown",
            "[STORAGE]   refusing to write rather than assume the range is clear".into(),
        ),
        Refusal::LayoutUnverifiable { image_sectors, boot_area_sectors } => (
            "the boot area this device advertises cannot hold the running kernel image, so where the firmware loads it from is not established",
            format!(
                "[STORAGE]   image needs {} sectors, device advertises {} as bootable",
                image_sectors, boot_area_sectors
            ),
        ),
        Refusal::OverlapsBootMedia { boot_area_end } => (
            "the target range is inside the bootable region the firmware loads this kernel from",
            format!(
                "[STORAGE]   target LBA {}..{} overlaps boot media LBA 0..{}",
                target.start, target.end, boot_area_end
            ),
        ),
    };
    let consequence = match boot {
        Some(area) if area.is_boot_media() => format!(
            "[STORAGE]   this is the boot device, so no second data disk was selected; attach one (QEMU: -drive index=0,file=boot.img -drive index=1,file=data.img) for persistent storage"
        ),
        _ => "[STORAGE]   writes to persistent storage will not survive a reboot until a separate data device is available".into(),
    };
    RefusalNotice {
        lines: [
            format!(
                "[STORAGE] REFUSED {} on device '{}': {}",
                scope, model, reason
            ),
            image_line,
            detail.into(),
            consequence,
        ],
    }
}

/// Prints a refusal to the serial console and returns it so callers can map it
/// onto their own error type.
pub fn announce_refusal<D: BlockDevice + ?Sized>(
    scope: &str,
    device: &D,
    target: SectorRange,
    boot: Option<&BootArea>,
    refusal: Refusal,
) -> RefusalNotice {
    let notice = refuse_notice(scope, device.model(), target, boot, refusal);
    for line in notice.lines.iter() {
        crate::serial_println!("{}", line);
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Builds a real 64-bit ELF byte image with the given program headers, so
    /// the parser is exercised against the layout it actually walks.
    struct SyntheticElf {
        bytes: Vec<u8>,
        hdr_vaddr: u64,
    }

    struct Phdr {
        p_type: u32,
        p_offset: u64,
        p_vaddr: u64,
        p_paddr: u64,
        p_filesz: u64,
        p_memsz: u64,
    }

    impl SyntheticElf {
        fn build(hdr_vaddr: u64, phdrs: &[Phdr], file_bytes: u64) -> Self {
            let phoff = 64u64;
            let table = phdrs.len() as u64 * 56;
            let mut bytes = vec![0u8; (phoff + table) as usize];
            bytes[0..4].copy_from_slice(&ELF_MAGIC);
            bytes[EI_CLASS] = ELFCLASS64;
            bytes[EI_DATA] = ELFDATA2LSB;
            bytes[E_TYPE..E_TYPE + 2].copy_from_slice(&3u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
            bytes[E_PHOFF..E_PHOFF + 8].copy_from_slice(&phoff.to_le_bytes());
            bytes[E_PHENTSIZE..E_PHENTSIZE + 2].copy_from_slice(&56u16.to_le_bytes());
            bytes[E_PHNUM..E_PHNUM + 2].copy_from_slice(&(phdrs.len() as u16).to_le_bytes());
            for (i, p) in phdrs.iter().enumerate() {
                let o = (phoff + i as u64 * 56) as usize;
                bytes[o + PH_TYPE..o + PH_TYPE + 4].copy_from_slice(&p.p_type.to_le_bytes());
                bytes[o + PH_OFFSET..o + PH_OFFSET + 8].copy_from_slice(&p.p_offset.to_le_bytes());
                bytes[o + PH_VADDR..o + PH_VADDR + 8].copy_from_slice(&p.p_vaddr.to_le_bytes());
                bytes[o + PH_PADDR..o + PH_PADDR + 8].copy_from_slice(&p.p_paddr.to_le_bytes());
                bytes[o + PH_FILESZ..o + PH_FILESZ + 8].copy_from_slice(&p.p_filesz.to_le_bytes());
                bytes[o + PH_MEMSZ..o + PH_MEMSZ + 8].copy_from_slice(&p.p_memsz.to_le_bytes());
            }
            assert!(file_bytes >= bytes.len() as u64);
            Self { bytes, hdr_vaddr }
        }

        fn parse(&self) -> Option<ImageExtent> {
            // WHY the subtraction: `parse_extent_at` reports addresses as the
            // image is mapped, while the byte image is indexed from zero. Relocating
            // the base has to move the reported extent, which is the property the
            // layout-independence test below depends on.
            parse_extent_at(self.hdr_vaddr, &|addr| {
                addr.checked_sub(self.hdr_vaddr).and_then(|i| self.bytes.get(i as usize).copied())
            })
        }
    }

    /// WHY `base` is accepted and ignored: the phdr `p_paddr` already carries
    /// the link-time physical address, so the run-time base is added once, at
    /// `hdr_vaddr`. Keeping the argument makes each call site read as the ELF it
    /// describes.
    fn load(_base: u64, vaddr: u64, memsz: u64, filesz: u64, offset: u64) -> Phdr {
        Phdr { p_type: PT_LOAD, p_offset: offset, p_vaddr: vaddr, p_paddr: vaddr, p_filesz: filesz, p_memsz: memsz }
    }

    /// A boot sector with two partitions, matching the shape a packaged image
    /// has: a small loader partition followed by the FAT partition holding the
    /// kernel.
    fn mbr_boot_sector(first_lba: u32, sectors: u32, second_lba: u32, second_sectors: u32) -> [u8; SECTOR_SIZE] {
        let mut s = [0u8; SECTOR_SIZE];
        for (i, (lba, count)) in [(first_lba, sectors), (second_lba, second_sectors)].iter().enumerate() {
            let o = MBR_PARTITION_TABLE + i * MBR_PARTITION_ENTRY;
            s[o + MBR_ENTRY_STATUS] = MBR_BOOT_ACTIVE;
            s[o + MBR_ENTRY_TYPE] = 0x0C;
            s[o + MBR_ENTRY_START_LBA..o + MBR_ENTRY_START_LBA + 4].copy_from_slice(&lba.to_le_bytes());
            s[o + MBR_ENTRY_SECTORS..o + MBR_ENTRY_SECTORS + 4].copy_from_slice(&count.to_le_bytes());
        }
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    /// An extent matching the packaged boot image's kernel ELF, whose program
    /// headers were read out of `target/inter-os-bios.img` at LBA 1551. Using
    /// the real geometry is what makes `file_sectors` comparable with a real
    /// device's boot area in the tests below.
    fn kernel_image() -> ImageExtent {
        let elf = SyntheticElf::build(
            0x40_0000,
            &[
                Phdr { p_type: 2, p_offset: 0x2323B0, p_vaddr: 0x2323B0, p_paddr: 0x2323B0, p_filesz: 0xE0, p_memsz: 0xE0 },
                load(0x40_0000, 0x0, 0x11F134, 0x11F134, 0x0),
                load(0x40_0000, 0x120140, 0xD6BCB, 0xD6BCB, 0x11F140),
                load(0x40_0000, 0x1F7D10, 0x6CA7A, 0x3B370, 0x1F5D10),
            ],
            0x231080,
        );
        elf.parse().expect("synthetic kernel image must parse")
    }

    #[test]
    fn extent_comes_from_program_headers_not_a_constant() {
        // The same program headers relocated to a different base must produce a
        // different extent, which is what makes the guard layout-independent.
        let at_4m = kernel_image();
        let at_20m = SyntheticElf::build(
            0x140_0000,
            &[
                load(0x140_0000, 0, 0x11B5B4, 0x11B5B4, 0),
                load(0x140_0000, 0x11C5C0, 0xD6BCB, 0xD6BCB, 0x11B5C0),
            ],
            0x1F2190,
        )
        .parse()
        .expect("synthetic image must parse");

        assert_eq!(at_4m.vaddr_start, 0x40_0000);
        assert_eq!(at_20m.vaddr_start, 0x140_0000);
        assert_eq!(at_4m.vaddr_start, at_4m.paddr_start);
        assert!(at_4m.paddr_is_identity());
        // Highest `p_vaddr + p_memsz` across the three segments.
        assert_eq!(at_4m.vaddr_end, 0x40_0000 + 0x26478A);
        // Highest `p_offset + p_filesz` across them, which is the on-disk length
        // the packaged image's kernel ELF actually measures: 2,297,984 bytes.
        assert_eq!(at_4m.file_bytes, 0x231080);
        assert_eq!(at_4m.file_sectors(), 4489);
    }

    #[test]
    fn non_elf_header_is_rejected() {
        let mut junk = vec![0u8; 4096];
        junk[0..4].copy_from_slice(b"NOT!");
        assert_eq!(parse_extent_at(0, &|a| junk.get(a as usize).copied()), None);
    }

    #[test]
    fn implausible_program_header_geometry_is_rejected() {
        // e_phnum far past the bound.
        let mut elf = SyntheticElf::build(0, &[load(0, 0, 16, 16, 0)], 4096);
        elf.bytes[E_PHNUM..E_PHNUM + 2].copy_from_slice(&900u16.to_le_bytes());
        assert_eq!(elf.parse(), None);
        // A program header table beyond the scan bound.
        let mut elf = SyntheticElf::build(0, &[load(0, 0, 16, 16, 0)], 4096);
        elf.bytes[E_PHOFF..E_PHOFF + 8].copy_from_slice(&0x9_0000u64.to_le_bytes());
        assert_eq!(elf.parse(), None);
        // An entry size smaller than one program header.
        let mut elf = SyntheticElf::build(0, &[load(0, 0, 16, 16, 0)], 4096);
        elf.bytes[E_PHENTSIZE..E_PHENTSIZE + 2].copy_from_slice(&32u16.to_le_bytes());
        assert_eq!(elf.parse(), None);
        // A machine type that is not this one.
        let mut elf = SyntheticElf::build(0, &[load(0, 0, 16, 16, 0)], 4096);
        elf.bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert_eq!(elf.parse(), None);
    }

    #[test]
    fn boot_area_follows_the_partition_table() {
        let sector = mbr_boot_sector(1, 961, 962, 8192);
        let area = parse_boot_area(&sector);
        assert!(area.is_boot_media());
        assert_eq!(area.active_entries, 2);
        assert_eq!(area.first_lba, 1);
        assert_eq!(area.end_lba, 962 + 8192);
    }

    #[test]
    fn unsigned_device_is_not_boot_media() {
        let area = parse_boot_area(&[0u8; SECTOR_SIZE]);
        assert!(!area.is_boot_media());
        assert_eq!(area.end_lba, 0);
    }

    #[test]
    fn format_over_the_boot_image_is_refused() {
        // The packaged layout, measured out of `target/inter-os-bios.img`:
        // 9154 sectors of media, an MBR whose partitions end at LBA 9154, a
        // kernel ELF of 4489 sectors sitting inside the FAT partition at LBA
        // 1551, and a canonical InterFS region of 2754 sectors from LBA 4096.
        let sector = mbr_boot_sector(1, 961, 962, 8192);
        let area = parse_boot_area(&sector);
        assert_eq!(area.end_lba, 9154);
        let image = kernel_image();
        assert_eq!(image.file_sectors(), 4489);
        // WHY this assertion matters: it is the check that keeps the diagnostic
        // specific. The image does fit inside the advertised boot area, so the
        // refusal can honestly name the overlap rather than shrugging with
        // "layout unverifiable".
        assert!(image.file_sectors() <= area.end_lba as u64);

        let target = SectorRange::new(4096, 2754).expect("range must not wrap");
        assert_eq!(
            evaluate_writes(Some(&image), &area, 9154, target),
            WriteVerdict::Refused(Refusal::OverlapsBootMedia { boot_area_end: 9154 })
        );
    }

    #[test]
    fn refusal_tracks_the_partition_table_not_a_fixed_lba() {
        let image = kernel_image();
        // WHY these partition sizes: the boot area has to be able to hold the
        // running image, otherwise the guard correctly reports the layout as
        // unverifiable before it ever reaches the overlap test.
        let area = parse_boot_area(&mbr_boot_sector(1, 961, 962, 4096));
        assert_eq!(area.end_lba, 5058);
        assert!(image.file_sectors() <= area.end_lba as u64);

        // A layout whose base is past the boot area is permitted, so the same
        // base the kernel uses today would be allowed here.
        assert_eq!(
            evaluate_writes(Some(&image), &area, 9154, SectorRange::new(8192, 1024).expect("range")),
            WriteVerdict::Permitted
        );
        // The canonical base is inside this boot area and is refused.
        assert_eq!(
            evaluate_writes(Some(&image), &area, 9154, SectorRange::new(4096, 512).expect("range")),
            WriteVerdict::Refused(Refusal::OverlapsBootMedia { boot_area_end: 5058 })
        );

        // Repack the image so the boot area now reaches base 8192. The refusal
        // has to follow the partition table rather than remembering 8192 was
        // allowed a moment ago.
        let wider = parse_boot_area(&mbr_boot_sector(1, 4096, 4097, 4096));
        assert_eq!(wider.end_lba, 8193);
        assert_eq!(
            evaluate_writes(Some(&image), &wider, 9154, SectorRange::new(8192, 1024).expect("range")),
            WriteVerdict::Refused(Refusal::OverlapsBootMedia { boot_area_end: 8193 })
        );
    }

    #[test]
    fn a_range_ending_exactly_at_the_boot_area_is_outside_it() {
        let area = parse_boot_area(&mbr_boot_sector(1, 961, 962, 4096));
        let image = kernel_image();
        assert_eq!(
            evaluate_writes(Some(&image), &area, 9154, SectorRange::new(5058, 64).expect("range")),
            WriteVerdict::Permitted
        );
        assert_eq!(
            evaluate_writes(Some(&image), &area, 9154, SectorRange::new(5057, 64).expect("range")),
            WriteVerdict::Refused(Refusal::OverlapsBootMedia { boot_area_end: 5058 })
        );
    }

    #[test]
    fn a_device_without_a_boot_signature_is_usable() {
        let area = parse_boot_area(&[0u8; SECTOR_SIZE]);
        let image = kernel_image();
        let target = SectorRange::new(4096, 32809).expect("range must not wrap");
        assert_eq!(evaluate_writes(Some(&image), &area, 131072, target), WriteVerdict::Permitted);
    }

    #[test]
    fn an_undeterminable_self_image_refuses() {
        let sector = mbr_boot_sector(1, 961, 962, 8192);
        let area = parse_boot_area(&sector);
        let target = SectorRange::new(100_000, 128).expect("range must not wrap");
        assert_eq!(
            evaluate_writes(None, &area, 131072, target),
            WriteVerdict::Refused(Refusal::SelfImageUnknown)
        );
        // ...and refuses on a non-boot device too, because the same argument
        // applies: nothing can prove the range is clear.
        assert_eq!(
            evaluate_writes(None, &parse_boot_area(&[0u8; SECTOR_SIZE]), 131072, target),
            WriteVerdict::Refused(Refusal::SelfImageUnknown)
        );
    }

    #[test]
    fn a_boot_area_smaller_than_the_image_is_unverifiable() {
        let sector = mbr_boot_sector(1, 4, 5, 8);
        let area = parse_boot_area(&sector);
        let image = kernel_image();
        let target = SectorRange::new(4096, 64).expect("range must not wrap");
        assert_eq!(
            evaluate_writes(Some(&image), &area, 131072, target),
            WriteVerdict::Refused(Refusal::LayoutUnverifiable {
                image_sectors: image.file_sectors(),
                boot_area_sectors: 13,
            })
        );
    }

    #[test]
    fn a_signature_with_no_partitions_is_unverifiable() {
        let mut sector = [0u8; SECTOR_SIZE];
        sector[510] = 0x55;
        sector[511] = 0xAA;
        let area = parse_boot_area(&sector);
        let image = kernel_image();
        let target = SectorRange::new(4096, 64).expect("range must not wrap");
        assert!(matches!(
            evaluate_writes(Some(&image), &area, 131072, target),
            WriteVerdict::Refused(Refusal::LayoutUnverifiable { boot_area_sectors: 0, .. })
        ));
    }

    #[test]
    fn the_console_names_the_device_range_and_consequence() {
        let sector = mbr_boot_sector(1, 961, 962, 8192);
        let area = parse_boot_area(&sector);
        let target = SectorRange::new(4096, 2754).expect("range must not wrap");
        let notice = refuse_notice(
            "InterFS format",
            "QEMU HARDDISK",
            target,
            Some(&area),
            Refusal::OverlapsBootMedia { boot_area_end: 9154 },
        );
        let text = notice.text();
        assert!(text.contains("REFUSED InterFS format on device 'QEMU HARDDISK'"), "{}", text);
        assert!(text.contains("target LBA 4096..6850 overlaps boot media LBA 0..9154"), "{}", text);
        assert!(text.contains("this kernel image: 4489 sectors on disk"), "{}", text);
        assert!(text.contains("no second data disk was selected"), "{}", text);
        assert!(text.contains("-drive index=1,file=data.img"), "{}", text);
        assert_eq!(notice.len(), 4);
    }

    #[test]
    fn an_unreadable_boot_sector_refuses_and_is_reported() {
        struct FailingDevice;
        impl BlockDevice for FailingDevice {
            fn read_sector(&mut self, _lba: u32, _buf: &mut [u8; SECTOR_SIZE]) -> Result<(), crate::storage::block::BlockDeviceError> {
                Err(crate::storage::block::BlockDeviceError::ReadError)
            }
            fn write_sector(&mut self, _lba: u32, _buf: &[u8; SECTOR_SIZE]) -> Result<(), crate::storage::block::BlockDeviceError> {
                Ok(())
            }
            fn sector_count(&self) -> u32 { 1024 }
            fn model(&self) -> &str { "unreadable" }
        }
        let target = SectorRange::new(64, 64).expect("range must not wrap");
        assert_eq!(
            evaluate_device_writes(&mut FailingDevice, target),
            WriteVerdict::Refused(Refusal::BootSectorUnreadable)
        );
        let notice = refuse_notice("persistent journal commit", "unreadable", target, None, Refusal::BootSectorUnreadable);
        assert!(notice.text().contains("LBA 0 could not be read"), "{}", notice.text());
    }
}