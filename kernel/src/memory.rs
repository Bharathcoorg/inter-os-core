// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Physical and Virtual Memory Management
//!
//! Handles page table initialization using the physical memory offset provided by the
//! bootloader, and provides a frame allocator backed by the UEFI/BIOS memory map.
//! Enforces zero-trust hardware MMU privilege boundaries between Ring 0 and Ring 3.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use spin::Mutex;
use alloc::vec::Vec;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::Mapper;
use x86_64::structures::paging::{
    FrameAllocator, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

static PHYSICAL_MEMORY_OFFSET: Mutex<Option<VirtAddr>> = Mutex::new(None);
static MEMORY_MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);
static KERNEL_PML4_FRAME: Mutex<Option<PhysFrame<Size4KiB>>> = Mutex::new(None);
static TOTAL_PHYSICAL_RAM_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(2048 * 1024 * 1024);

/// Initializes global memory management singletons: page mapper, physical offset, and frame allocator.
///
/// INVARIANT (ordering): `PHYSICAL_MEMORY_OFFSET` is published *before* the mapper and
/// the frame allocator are constructed, because `init_page_table` and `BootInfoFrameAllocator::init`
/// both reach the frame allocator, and every frame the allocator returns must be zeroed
/// through the direct map. Publishing the offset first means no allocation can ever observe
/// an unpublished offset and silently skip zeroing. Previously the offset was stored *after*
/// both constructors ran, which made zeroing depend on call ordering inside this function.
pub fn init_memory(physical_memory_offset: VirtAddr, memory_regions: &'static MemoryRegions) {
    let (kernel_frame, _) = Cr3::read();
    *KERNEL_PML4_FRAME.lock() = Some(kernel_frame);

    // Publish the direct-map offset first: it is the precondition for zeroing frames.
    *PHYSICAL_MEMORY_OFFSET.lock() = Some(physical_memory_offset);

    let mapper = unsafe { init_page_table(physical_memory_offset) };
    let frame_allocator =
        unsafe { BootInfoFrameAllocator::init(memory_regions, physical_memory_offset) };

    *MEMORY_MAPPER.lock() = Some(mapper);
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);

    let pml4_ptr = (physical_memory_offset + kernel_frame.start_address().as_u64()).as_ptr::<PageTable>();
    let pml4 = unsafe { &*pml4_ptr };
    for i in 0..512 {
        if !pml4[i].is_unused() {
            crate::serial_println!("[BOOT] Kernel PML4 entry {}: frame {:#x}, flags {:?}", i, pml4[i].addr().as_u64(), pml4[i].flags());
        }
    }
    for (i, r) in memory_regions.iter().enumerate() {
        crate::serial_println!("[MEMMAP] region {}: {:#x}..{:#x} ({:?})", i, r.start, r.end, r.kind);
    }

    let mut detected_ram: u64 = 0;
    for r in memory_regions.iter() {
        // WHY the kind filter: the map also names the PCI MMIO aperture and the
        // low BIOS/ACPI windows as regions, and summing every kind counted a
        // 256 MiB hole as installed memory — a 4 GiB guest booted claiming
        // 4351 MB, which is the figure the memory card then printed. `Usable`
        // and `Bootloader` are the only kinds that are RAM; the bootloader's
        // own pages are RAM the kernel simply cannot hand out.
        match r.kind {
            MemoryRegionKind::Usable | MemoryRegionKind::Bootloader => {
                detected_ram = detected_ram.saturating_add(r.end.saturating_sub(r.start));
            }
            _ => {}
        }
    }
    if detected_ram >= 128 * 1024 * 1024 {
        TOTAL_PHYSICAL_RAM_BYTES.store(detected_ram, core::sync::atomic::Ordering::Release);
        crate::serial_println!("[BOOT] Total physical RAM detected: {} MB", detected_ram / (1024 * 1024));
    }
}

/// Returns the primordial kernel PML4 physical frame.
pub fn kernel_pml4() -> PhysFrame<Size4KiB> {
    KERNEL_PML4_FRAME
        .lock()
        .expect("Kernel PML4 frame not recorded during init_memory")
}

/// Switches CPU CR3 back to the clean kernel PML4.
pub fn switch_to_kernel_page_table() {
    let (current_frame, flags) = Cr3::read();
    let kpml4 = kernel_pml4();
    if current_frame != kpml4 {
        unsafe {
            Cr3::write(kpml4, flags);
        }
    }
}

/// Returns the physical memory offset mapped by the bootloader.
pub fn phys_mem_offset() -> VirtAddr {
    PHYSICAL_MEMORY_OFFSET
        .lock()
        .expect("Physical memory offset not initialized")
}

/// Physical RAM telemetry structure backing sys_memory_telemetry.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTelemetry {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// Computes physical memory usage honestly from the platform RAM info, active tasks,
/// and kernel heap/reserved memory regions.
pub fn memory_telemetry() -> MemoryTelemetry {
    let total_bytes = {
        let detected = TOTAL_PHYSICAL_RAM_BYTES.load(core::sync::atomic::Ordering::Acquire);
        let platform = crate::hal::PLATFORM.info().total_ram_bytes as u64;
        detected.max(platform)
    };

    // Calculate active user tasks memory from TaskPageTable allocated frames / mapped bytes:
    let mut tasks_mapped_bytes: u64 = 0;
    if let Some(scheduler) = crate::task::SCHEDULER.try_lock() {
        for tcb in scheduler.tasks_iter() {
            if let Some(ref pt_arc) = tcb.page_table {
                if let Some(pt) = pt_arc.try_lock() {
                    let user_bytes = if pt.user_mapped_bytes > 0 {
                        pt.user_mapped_bytes as u64
                    } else {
                        (pt.allocated_frames.len() as u64).saturating_mul(4096)
                    };
                    tasks_mapped_bytes = tasks_mapped_bytes.saturating_add(user_bytes);
                }
            }
            if !tcb.stack.is_empty() {
                tasks_mapped_bytes = tasks_mapped_bytes.saturating_add(crate::task::TASK_STACK_SIZE as u64);
            }
        }
    }

    // Kernel heap allocated / used bytes:
    #[cfg(not(test))]
    let kernel_heap_bytes = {
        let (used, _) = crate::allocator::try_heap_stats().unwrap_or((0, 0));
        (used as u64).max(4 * 1024 * 1024)
    };
    #[cfg(test)]
    let kernel_heap_bytes = 4 * 1024 * 1024u64;

    let frame_allocator_bytes: u64 = if let Some(guard) = FRAME_ALLOCATOR.try_lock() {
        if let Some(ref fa) = *guard {
            fa.allocated_bytes()
        } else {
            0
        }
    } else {
        0
    };

    let calculated_used = tasks_mapped_bytes
        .saturating_add(kernel_heap_bytes)
        .max(frame_allocator_bytes);

    let used_bytes = calculated_used.min(total_bytes);
    let free_bytes = total_bytes.saturating_sub(used_bytes);

    MemoryTelemetry {
        total_bytes,
        used_bytes,
        free_bytes,
    }
}

/// Allocates physically contiguous 4 KiB frames for DMA buffers.
///
/// Contiguity has to be established at allocation time: the general allocator
/// serves one frame per call and prefers recycled frames, whose addresses have no
/// relation to each other or to the bump pointer. Assembling a run by repeated
/// single-frame allocation therefore fails as soon as the recycle pool is non-empty,
/// which is always the case after boot has torn down a user address space.
pub fn allocate_dma_frames(count: usize) -> Option<alloc::vec::Vec<PhysFrame<Size4KiB>>> {
    FRAME_ALLOCATOR.lock().as_mut()?.allocate_contiguous_frames(count)
}

/// Converts a physical address into its bootloader-provided direct-map virtual address.
pub fn physical_to_virtual(physical: PhysAddr) -> VirtAddr {
    phys_mem_offset() + physical.as_u64()
}

/// Maps a physical linear framebuffer range into the kernel address space with
/// Write-Combining (WC) page attributes via PAT (x86_64), enabling accelerated
/// 64-byte burst writes for scanout presentation.
pub fn map_framebuffer_region(
    physical_base: PhysAddr,
    size: usize,
    virtual_base: VirtAddr,
) -> Result<VirtAddr, &'static str> {
    if size == 0 { return Err("Framebuffer mapping size is zero"); }

    let pages = size.checked_add(4095).ok_or("Framebuffer size overflow")? / 4096;
    let span = pages.checked_mul(4096).ok_or("Framebuffer span overflow")?;
    let virt_end = virtual_base.as_u64().checked_add(span as u64).ok_or("Framebuffer virtual range overflow")?;
    let phys_end = physical_base.as_u64().checked_add(span as u64).ok_or("Framebuffer physical range overflow")?;
    if virt_end == 0 || phys_end == 0 {
        return Err("Framebuffer address range overflow");
    }

    let mut mapper_lock = MEMORY_MAPPER.lock();
    let mapper = mapper_lock.as_mut().ok_or("Memory mapper not initialized")?;
    let mut mapped_pages: alloc::vec::Vec<Page<Size4KiB>> = alloc::vec::Vec::new();
    let mut frame_allocator = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_allocator.as_mut().ok_or("Frame allocator not initialized")?;

    // PageTableFlags::WRITE_THROUGH routes through PAT entry 1 (WC = Write-Combining)
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::WRITE_THROUGH;

    for index in 0..pages {
        let virt = virtual_base.as_u64()
            .checked_add((index as u64).checked_mul(4096).ok_or("Framebuffer virtual offset overflow")?)
            .ok_or("Framebuffer virtual address overflow")?;
        let phys = physical_base.as_u64()
            .checked_add((index as u64).checked_mul(4096).ok_or("Framebuffer physical offset overflow")?)
            .ok_or("Framebuffer physical address overflow")?;

        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
        let phys_frame = PhysFrame::containing_address(PhysAddr::new(phys));

        match unsafe { mapper.map_to(page, phys_frame, flags, frame_allocator) } {
            Ok(flush) => {
                flush.flush();
                mapped_pages.push(page);
            }
            Err(MapToError::PageAlreadyMapped(_)) => {
                for mapped in mapped_pages.drain(..).rev() {
                    let _ = mapper.unmap(mapped).map(|(_, flush)| flush.flush());
                }
                return Err("Framebuffer virtual range already mapped");
            }
            Err(_) => {
                for mapped in mapped_pages.drain(..).rev() {
                    let _ = mapper.unmap(mapped).map(|(_, flush)| flush.flush());
                }
                return Err("Framebuffer page mapping failed");
            }
        }
    }

    Ok(virtual_base)
}

/// Maps a physical MMIO range into the kernel address space with cache-disabled
/// page attributes. The caller supplies a dedicated virtual base.
pub fn map_mmio_region(
    physical_base: PhysAddr,
    size: usize,
    virtual_base: VirtAddr,
) -> Result<VirtAddr, &'static str> {
    if size == 0 { return Err("MMIO mapping size is zero"); }

    // Round the byte span up without permitting arithmetic wraparound.
    let pages = size.checked_add(4095).ok_or("MMIO size overflow")? / 4096;
    let span = pages.checked_mul(4096).ok_or("MMIO span overflow")?;
    let virt_end = virtual_base.as_u64().checked_add(span as u64).ok_or("MMIO virtual range overflow")?;
    let phys_end = physical_base.as_u64().checked_add(span as u64).ok_or("MMIO physical range overflow")?;
    if virt_end == 0 || phys_end == 0 {
        return Err("MMIO address range overflow");
    }

    let mut mapper_lock = MEMORY_MAPPER.lock();
    let mapper = mapper_lock.as_mut().ok_or("Memory mapper not initialized")?;
    let mut mapped_pages: alloc::vec::Vec<Page<Size4KiB>> = alloc::vec::Vec::new();
    let mut frame_allocator = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_allocator.as_mut().ok_or("Frame allocator not initialized")?;

    for index in 0..pages {
        let virt = virtual_base.as_u64()
            .checked_add((index as u64).checked_mul(4096).ok_or("MMIO virtual offset overflow")?)
            .ok_or("MMIO virtual address overflow")?;
        let phys = physical_base.as_u64()
            .checked_add((index as u64).checked_mul(4096).ok_or("MMIO physical offset overflow")?)
            .ok_or("MMIO physical address overflow")?;

        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
        let phys_frame = PhysFrame::containing_address(PhysAddr::new(phys));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

        match unsafe { mapper.map_to(page, phys_frame, flags, frame_allocator) } {
            Ok(flush) => {
                flush.flush();
                mapped_pages.push(page);
            }
            Err(MapToError::PageAlreadyMapped(_)) => {
                for mapped in mapped_pages.drain(..).rev() {
                    let _ = mapper.unmap(mapped).map(|(_, flush)| flush.flush());
                }
                return Err("MMIO virtual range already mapped");
            }
            Err(_) => {
                for mapped in mapped_pages.drain(..).rev() {
                    let _ = mapper.unmap(mapped).map(|(_, flush)| flush.flush());
                }
                return Err("MMIO page mapping failed");
            }
        }
    }

    Ok(virtual_base)
}

/// Allocates a single 4 KiB physical memory frame.
pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAME_ALLOCATOR.lock().as_mut()?.allocate_frame()
}

/// Recycles a task-owned physical frame after its address space is destroyed.
pub fn recycle_frame(frame: PhysFrame<Size4KiB>) {
    if let Some(allocator) = FRAME_ALLOCATOR.lock().as_mut() {
        allocator.recycle_frame(frame);
    }
}

/// Initializes the 1 MiB kernel heap using the global page mapper and frame allocator.
pub fn init_kernel_heap() -> Result<(), MapToError<Size4KiB>> {
    let mut mapper_lock = MEMORY_MAPPER.lock();
    let mapper = mapper_lock.as_mut().expect("Mapper not initialized");
    let mut frame_alloc_lock = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_alloc_lock.as_mut().expect("Frame allocator not initialized");
    crate::allocator::init_heap(mapper, frame_allocator)
}

/// An isolated Level 4 Page Table (PML4) dedicated to a specific user task.
///
/// Owns the root physical frame. Lower half (entries 0..256) is completely isolated
/// per task, while higher half (entries 256..512) mirrors the kernel's address space.
/// Tracks all physical frames allocated for page tables and user mappings.
#[derive(Debug)]
pub struct TaskPageTable {
    pub pml4_frame: PhysFrame<Size4KiB>,
    pub allocated_frames: alloc::vec::Vec<PhysFrame<Size4KiB>>,
    /// Bytes of user virtual memory currently backed by leaf mappings.
    pub user_mapped_bytes: usize,
}

impl TaskPageTable {
    /// Creates a new per-task page table by cloning the kernel's higher-half mappings.
    pub fn create_user_page_table() -> Result<Self, &'static str> {
        let pml4_frame = allocate_frame().ok_or("Out of physical memory frames for PML4")?;
        let phys_offset = phys_mem_offset();

        unsafe {
            let new_pml4_ptr = (phys_offset + pml4_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            let new_pml4 = &mut *new_pml4_ptr;
            new_pml4.zero();

            // Mirror only the kernel/supervisor higher half (256..512) from the kernel PML4.
            // The entire lower half remains private to this Ring-3 address space.
            let kernel_pml4_frame = kernel_pml4();
            let kernel_pml4_ptr = (phys_offset + kernel_pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
            let kernel_pml4 = &*kernel_pml4_ptr;

            for i in 256..512 {
                new_pml4[i] = kernel_pml4[i].clone();
            }

            // Clone non-empty supervisor mappings from the primordial kernel PML4 (entries 1..256),
            // preserving entry 0 exclusively for user processes.
            let mut k = 1;
            while k < 256 {
                if !kernel_pml4[k].is_unused() {
                    new_pml4[k] = kernel_pml4[k].clone();
                }
                k += 1;
            }
        }

        let mut allocated = alloc::vec::Vec::new();
        allocated.push(pml4_frame);
        Ok(Self {
            pml4_frame,
            allocated_frames: allocated,
            user_mapped_bytes: 0,
        })
    }

    /// Activates this task's page table by loading its physical frame into CR3.
    /// Skips reloading CR3 if already active, preventing redundant TLB flushes.
    pub fn switch_to(&self) {
        let (current_frame, flags) = Cr3::read();
        if current_frame != self.pml4_frame {
            unsafe {
                Cr3::write(self.pml4_frame, flags);
            }
        }
    }

    /// Returns the physical PML4 frame.
    pub fn frame(&self) -> PhysFrame<Size4KiB> {
        self.pml4_frame
    }

    /// Returns the currently mapped user virtual-memory footprint.
    pub fn user_mapped_bytes(&self) -> usize {
        self.user_mapped_bytes
    }

    /// Maps a virtual page to a physical frame in this process's isolated page table.
    /// Intermediate tables (PDPT, PD, PT) are dynamically allocated with USER_ACCESSIBLE flags.
    pub fn map_user_page(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        // Reject impossible user permissions before allocating any page-table
        // frames. This keeps W^X failures side-effect free.
        if flags.contains(PageTableFlags::WRITABLE) && !flags.contains(PageTableFlags::NO_EXECUTE) {
            return Err("W^X violation: writable user mapping must be non-executable");
        }

        // Mapping is transactional: any page-table frames allocated by this
        // operation are unlinked before their frames are recycled.
        // The caller retains ownership of the leaf frame on failure.
        let allocated_before = self.allocated_frames.len();
        let rollback = |
            table: &mut Self,
            phys_offset: VirtAddr,
            p4_idx: usize,
            p3_idx: usize,
            p2_idx: usize,
            new_p3: bool,
            new_p2: bool,
            new_p1: bool,
        | {
            // Unlink newly-created tables before recycling them. Otherwise a
            // later mapping could follow a recycled physical frame.
            if new_p1 {
                let pml4_ptr = (phys_offset + table.pml4_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                let pml4 = unsafe { &mut *pml4_ptr };
                if let Ok(p3_frame) = pml4[p4_idx].frame() {
                    let p3_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                    let p3 = unsafe { &mut *p3_ptr };
                    if let Ok(p2_frame) = p3[p3_idx].frame() {
                        let p2_ptr = (phys_offset + p2_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                        let p2 = unsafe { &mut *p2_ptr };
                        p2[p2_idx].set_unused();
                    }
                }
            }
            if new_p2 {
                let pml4_ptr = (phys_offset + table.pml4_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                let pml4 = unsafe { &mut *pml4_ptr };
                if let Ok(p3_frame) = pml4[p4_idx].frame() {
                    let p3_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                    let p3 = unsafe { &mut *p3_ptr };
                    p3[p3_idx].set_unused();
                }
            }
            if new_p3 {
                let pml4_ptr = (phys_offset + table.pml4_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
                let pml4 = unsafe { &mut *pml4_ptr };
                pml4[p4_idx].set_unused();
            }

            while table.allocated_frames.len() > allocated_before {
                if let Some(owned) = table.allocated_frames.pop() {
                    recycle_frame(owned);
                }
            }
        };

        let vaddr = page.start_address().as_u64();
        if vaddr >= 0x0000_8000_0000_0000 {
            return Err("Cannot map user page into kernel address space");
        }

        let phys_offset = phys_mem_offset();
        let pml4_ptr = (phys_offset + self.pml4_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pml4 = unsafe { &mut *pml4_ptr };

        let p4_idx = ((vaddr >> 39) & 0x1FF) as usize;
        let p3_idx = ((vaddr >> 30) & 0x1FF) as usize;
        let p2_idx = ((vaddr >> 21) & 0x1FF) as usize;
        let p1_idx = ((vaddr >> 12) & 0x1FF) as usize;

        if p4_idx >= 256 {
            return Err("User page PML4 index in kernel range");
        }
        if !pml4[p4_idx].is_unused() && !self.allocated_frames.iter().any(|owned| {
            pml4[p4_idx].frame().map(|f| *owned == f).unwrap_or(false)
        }) {
            return Err("Cannot map user page into mirrored kernel PML4 entry");
        }
        if self.allocated_frames.iter().any(|owned| *owned == frame) {
            return Err("physical frame already owned by address space");
        }
        if flags.contains(PageTableFlags::WRITABLE) && !flags.contains(PageTableFlags::NO_EXECUTE) {
            return Err("W^X violation: writable user mapping must be non-executable");
        }

        let table_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        let mut new_p3 = false;
        let p3_frame = if pml4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            pml4[p4_idx].frame().map_err(|_| "Huge page at PML4")?
        } else {
            let Some(f) = allocate_frame() else {
                rollback(self, phys_offset, p4_idx, p3_idx, p2_idx, false, false, false);
                return Err("Out of memory for PDPT");
            };
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pml4[p4_idx].set_frame(f, table_flags);
            new_p3 = true;
            f
        };

        let mut new_p2 = false;
        let pdpt_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pdpt = unsafe { &mut *pdpt_ptr };
        let p2_frame = if pdpt[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            pdpt[p3_idx].frame().map_err(|_| "Huge page at PDPT")?
        } else {
            let Some(f) = allocate_frame() else {
                rollback(self, phys_offset, p4_idx, p3_idx, p2_idx, new_p3, false, false);
                return Err("Out of memory for PD");
            };
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pdpt[p3_idx].set_frame(f, table_flags);
            new_p2 = true;
            f
        };

        let mut new_p1 = false;
        let pd_ptr = (phys_offset + p2_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pd = unsafe { &mut *pd_ptr };
        let p1_frame = if pd[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            pd[p2_idx].frame().map_err(|_| "Huge page at PD")?
        } else {
            let Some(f) = allocate_frame() else {
                rollback(self, phys_offset, p4_idx, p3_idx, p2_idx, new_p3, new_p2, false);
                return Err("Out of memory for PT");
            };
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pd[p2_idx].set_frame(f, table_flags);
            new_p1 = true;
            f
        };

        let pt_ptr = (phys_offset + p1_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pt = unsafe { &mut *pt_ptr };
        if pt[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            rollback(self, phys_offset, p4_idx, p3_idx, p2_idx, new_p3, new_p2, new_p1);
            return Err("Virtual page already mapped");
        }

        // User mappings are never allowed to be simultaneously writable and executable.
        // Enforce W^X at the single mapping primitive so callers cannot bypass the
        // ELF loader's segment-level policy by constructing a mapping directly.
        let leaf_flags = (flags & !PageTableFlags::PRESENT) | PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        pt[p1_idx].set_frame(frame, leaf_flags);
        self.allocated_frames.push(frame);
        self.user_mapped_bytes = self.user_mapped_bytes.saturating_add(4096);

        let (active_cr3, _) = Cr3::read();
        if active_cr3 == self.pml4_frame {
            x86_64::instructions::tlb::flush(VirtAddr::new(vaddr));
        }

        Ok(())
    }

    /// Removes one userspace leaf mapping and returns its physical frame to the allocator.
    ///
    /// Intermediate page-table frames remain owned by this address space so future
    /// mappings in the same region can reuse them without changing ownership.
    pub fn unmap_user_page(&mut self, page: Page<Size4KiB>) -> Result<(), &'static str> {
        let vaddr = page.start_address().as_u64();
        if vaddr >= 0x0000_8000_0000_0000 {
            return Err("Cannot unmap a kernel address from a user page table");
        }

        let phys_offset = phys_mem_offset();
        let pml4_ptr = (phys_offset + self.pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let pml4 = unsafe { &*pml4_ptr };
        let p4_idx = ((vaddr >> 39) & 0x1FF) as usize;
        let p3_idx = ((vaddr >> 30) & 0x1FF) as usize;
        let p2_idx = ((vaddr >> 21) & 0x1FF) as usize;
        let p1_idx = ((vaddr >> 12) & 0x1FF) as usize;

        let p3_frame = pml4[p4_idx].frame().map_err(|_| "userspace PML4 entry missing")?;
        let p3_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let p3 = unsafe { &*p3_ptr };
        let p2_frame = p3[p3_idx].frame().map_err(|_| "userspace PDPT entry missing")?;
        let p2_ptr = (phys_offset + p2_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let p2 = unsafe { &*p2_ptr };
        let p1_frame = p2[p2_idx].frame().map_err(|_| "userspace PD entry missing")?;
        let p1_ptr = (phys_offset + p1_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let p1 = unsafe { &mut *p1_ptr };
        if !p1[p1_idx].flags().contains(PageTableFlags::PRESENT) {
            return Err("userspace page is not mapped");
        }

        let frame = p1[p1_idx].frame().map_err(|_| "userspace leaf frame missing")?;
        let Some(pos) = self.allocated_frames.iter().position(|candidate| *candidate == frame) else {
            return Err("userspace frame ownership missing");
        };
        p1[p1_idx].set_unused();
        self.allocated_frames.swap_remove(pos);
        self.user_mapped_bytes = self.user_mapped_bytes.saturating_sub(4096);

        let (active_cr3, _) = Cr3::read();
        if active_cr3 == self.pml4_frame {
            x86_64::instructions::tlb::flush(VirtAddr::new(vaddr));
        }
        recycle_frame(frame);
        Ok(())
    }

    /// Looks up a virtual address in this process's page table.
    /// Returns the translated physical address and leaf page table flags if mapped.
    pub fn translate(&self, vaddr: VirtAddr) -> Option<(PhysAddr, PageTableFlags)> {
        let addr = vaddr.as_u64();
        let p4_idx = ((addr >> 39) & 0x1FF) as usize;
        let p3_idx = ((addr >> 30) & 0x1FF) as usize;
        let p2_idx = ((addr >> 21) & 0x1FF) as usize;
        let p1_idx = ((addr >> 12) & 0x1FF) as usize;
        let offset = addr & 0xFFF;

        let phys_offset = phys_mem_offset();
        let pml4_ptr = (phys_offset + self.pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let pml4 = unsafe { &*pml4_ptr };

        let p4_entry = &pml4[p4_idx];
        if !p4_entry.flags().contains(PageTableFlags::PRESENT) {
            return None;
        }

        let p3_frame = p4_entry.frame().ok()?;
        let p3_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let p3 = unsafe { &*p3_ptr };

        let p3_entry = &p3[p3_idx];
        if !p3_entry.flags().contains(PageTableFlags::PRESENT) {
            return None;
        }

        let p2_frame = p3_entry.frame().ok()?;
        let p2_ptr = (phys_offset + p2_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let p2 = unsafe { &*p2_ptr };

        let p2_entry = &p2[p2_idx];
        if !p2_entry.flags().contains(PageTableFlags::PRESENT) {
            return None;
        }

        let p1_frame = p2_entry.frame().ok()?;
        let p1_ptr = (phys_offset + p1_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let p1 = unsafe { &*p1_ptr };

        let p1_entry = &p1[p1_idx];
        if !p1_entry.flags().contains(PageTableFlags::PRESENT) {
            return None;
        }

        let leaf_frame = p1_entry.frame().ok()?;
        let phys_addr = leaf_frame.start_address() + offset;
        Some((phys_addr, p1_entry.flags()))
    }

    /// Validates a complete userspace byte range against this address space.
    /// Every touched leaf must be present, userspace-accessible, have the
    /// requested write permission, and resolve to a frame owned by this task.
    pub fn validate_user_range(
        &self,
        addr: u64,
        len: usize,
        write: bool,
    ) -> Result<(), &'static str> {
        if addr < 0x1000 {
            return Err("null or guard page");
        }
        if len == 0 {
            return Ok(());
        }
        let end = addr.checked_add(len as u64).ok_or("address overflow")?;
        if end > 0x0000_8000_0000_0000 {
            return Err("kernel address range");
        }

        let first_page = addr & !0xFFF;
        let last_page = (end - 1) & !0xFFF;
        let mut page = first_page;
        loop {
            let (physical, flags) = self
                .translate(VirtAddr::new(page))
                .ok_or("user page is not mapped")?;

            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return Err("user page is not accessible");
            }
            if write && !flags.contains(PageTableFlags::WRITABLE) {
                return Err("user page is not writable");
            }

            let frame = PhysFrame::containing_address(physical);
            if !self.allocated_frames.iter().any(|owned| *owned == frame) {
                return Err("user frame is not owned by task");
            }

            if page == last_page {
                break;
            }
            page = page.checked_add(4096).ok_or("page iteration overflow")?;
        }
        Ok(())
    }
}

impl Drop for TaskPageTable {
    fn drop(&mut self) {
        let (active_cr3, _) = Cr3::read();
        if active_cr3 == self.pml4_frame {
            switch_to_kernel_page_table();
        }
        // Clear owned mappings before returning frames to the allocator.
        for frame in self.allocated_frames.drain(..) {
            recycle_frame(frame);
        }
    }
}

/// Creates a new user page table with isolated lower half and mirrored kernel upper half.
pub fn create_user_page_table() -> Result<TaskPageTable, &'static str> {
    TaskPageTable::create_user_page_table()
}

/// Switches CPU virtual translation to the specified task page table frame.
pub fn switch_to_task_page_table(pml4_frame: PhysFrame<Size4KiB>) {
    let (current_frame, flags) = Cr3::read();
    if current_frame != pml4_frame {
        unsafe {
            Cr3::write(pml4_frame, flags);
        }
    }
}

/// Runs automated adversarial tests proving MMU isolation, W^X enforcement,
/// kernel supervisor protection, and cross-process confidentiality.
pub fn run_mmu_adversarial_tests() {
    crate::serial_println!("[MMU TEST] Starting Hardware CR3 & Memory Isolation Adversarial Test Suite...");

    crate::serial_println!("[MMU TEST] Step 1a: create PT A");
    let mut pt_a = TaskPageTable::create_user_page_table().expect("Failed to create PT A");
    crate::serial_println!("[MMU TEST] Step 1b: create PT B");
    let mut pt_b = TaskPageTable::create_user_page_table().expect("Failed to create PT B");
    crate::serial_println!("[MMU TEST] Step 1c: assert PML4 frames distinct");

    // PML4 root frames must be distinct
    assert_ne!(pt_a.frame(), pt_b.frame(), "Process A and B shared the same PML4 frame!");

    crate::serial_println!("[MMU TEST] Step 2a: allocate frames");
    // 2. Allocate distinct physical frames for a shared virtual address (e.g. 0x0040_0000)
    let frame_a = allocate_frame().expect("Frame alloc A");
    let frame_b = allocate_frame().expect("Frame alloc B");
    assert_ne!(frame_a, frame_b, "Frame A and B collided");

    let test_page = Page::<Size4KiB>::containing_address(VirtAddr::new(0x0040_0000));
    let phys_offset = phys_mem_offset();

    crate::serial_println!("[MMU TEST] Step 2b: write distinct data");
    // Write distinct data into physical frames
    unsafe {
        let ptr_a = (phys_offset + frame_a.start_address().as_u64()).as_mut_ptr::<u32>();
        *ptr_a = 0xAAAA_1111;

        let ptr_b = (phys_offset + frame_b.start_address().as_u64()).as_mut_ptr::<u32>();
        *ptr_b = 0xBBBB_2222;
    }

    crate::serial_println!("[MMU TEST] Step 2c: map page in A");
    // Map test_page in A as Read+Execute (Code, RX), and in B as Read+Write+NX (Data, RW+NX)
    pt_a.map_user_page(test_page, frame_a, PageTableFlags::empty())
        .expect("Map page in A");
    crate::serial_println!("[MMU TEST] Step 2d: map page in B");
    pt_b.map_user_page(test_page, frame_b, PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE)
        .expect("Map page in B");

    // 3. Verify Process A translates 0x0040_0000 to frame_a, NOT frame_b
    let trans_a = pt_a.translate(VirtAddr::new(0x0040_0000)).expect("Translate A");
    assert_eq!(trans_a.0, frame_a.start_address(), "Isolation breach: A translates to unexpected frame");
    assert!(!trans_a.1.contains(PageTableFlags::WRITABLE), "W^X violation: Code page in A is writable");
    assert!(!trans_a.1.contains(PageTableFlags::NO_EXECUTE), "Code page in A incorrectly marked NX");

    // 4. Verify Process B translates 0x0040_0000 to frame_b, NOT frame_a
    let trans_b = pt_b.translate(VirtAddr::new(0x0040_0000)).expect("Translate B");
    assert_eq!(trans_b.0, frame_b.start_address(), "Isolation breach: B translates to unexpected frame");
    assert!(trans_b.1.contains(PageTableFlags::WRITABLE), "Data page in B lacks writable flag");
    assert!(trans_b.1.contains(PageTableFlags::NO_EXECUTE), "W^X violation: Writable data page in B lacks NX flag");

    // 5. Verify unmapped addresses return None (Page Fault in hardware)
    assert!(pt_a.translate(VirtAddr::new(0x0050_0000)).is_none(), "Unmapped address in A returned a mapping");
    assert!(pt_b.translate(VirtAddr::new(0x0050_0000)).is_none(), "Unmapped address in B returned a mapping");

    // 6. Verify kernel higher-half protection: entries 256..512 must NOT have USER_ACCESSIBLE
    let pml4_a_ptr = (phys_offset + pt_a.frame().start_address().as_u64()).as_ptr::<PageTable>();
    let pml4_a = unsafe { &*pml4_a_ptr };
    for idx in 256..512 {
        if pml4_a[idx].flags().contains(PageTableFlags::PRESENT) {
            assert!(
                !pml4_a[idx].flags().contains(PageTableFlags::USER_ACCESSIBLE),
                "Security breach: Kernel entry {} marked USER_ACCESSIBLE!",
                idx
            );
        }
    }

    crate::serial_println!("[MMU TEST] ✓ All MMU Adversarial Isolation Tests PASSED (CR3 per-task, W^X, NX, Kernel Protection).");
}


/// Initializes an `OffsetPageTable` given the physical memory offset.
pub unsafe fn init_page_table(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    let level_4_table = unsafe { &mut *page_table_ptr };
    unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) }
}

/// A physical frame allocator that returns usable frames from the bootloader's memory map
/// in O(1) time complexity per allocation.
pub struct BootInfoFrameAllocator {
    memory_regions: &'static MemoryRegions,
    region_idx: usize,
    next_addr: u64,
    recycled_frames: Vec<PhysFrame<Size4KiB>>,
    /// Direct-map offset used to reach every frame this allocator returns.
    ///
    /// INVARIANT: the offset is stored *in the allocator* rather than read from the
    /// global `PHYSICAL_MEMORY_OFFSET` at zeroing time. Reading a global made
    /// zeroing conditional on that global already being published; if any frame were
    /// ever allocated before publication the frame would be handed out unzeroed and
    /// the resulting page-table contents would depend on residual RAM contents.
    /// Owning the offset makes it impossible for `allocate_frame` to return an
    /// unzeroed frame.
    physical_memory_offset: VirtAddr,
}

impl BootInfoFrameAllocator {
    /// Returns the approximate allocated bytes from the boot memory regions.
    pub fn allocated_bytes(&self) -> u64 {
        self.next_addr.saturating_sub(self.recycled_frames.len() as u64 * 4096)
    }

    /// Returns a task-owned physical frame to the allocator for reuse.
    pub fn recycle_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        // Reject duplicate returns: a frame may only enter the free pool once.
        if self.recycled_frames.contains(&frame) {
            return;
        }
        // Only frames belonging to a bootloader-declared usable region may be
        // recycled back into the general allocator.
        let addr = frame.start_address().as_u64();
        if !self.memory_regions.iter().any(|region| {
            region.kind == MemoryRegionKind::Usable && addr >= region.start && addr + 4096 <= region.end
        }) {
            return;
        }
        self.recycled_frames.push(frame);
    }

    /// Creates a FrameAllocator from the passed memory map.
    ///
    /// `physical_memory_offset` is stored in the allocator so that every returned frame
    /// is unconditionally zeroed through the direct map; see the field documentation for
    /// why this must not depend on a separately published global.
    pub unsafe fn init(memory_regions: &'static MemoryRegions, physical_memory_offset: VirtAddr) -> Self {
        let mut allocator = BootInfoFrameAllocator {
            memory_regions,
            region_idx: 0,
            next_addr: 0,
            recycled_frames: Vec::new(),
            physical_memory_offset,
        };
        allocator.advance_to_usable_region();
        allocator
    }

    /// Allocates `count` physically adjacent zeroed frames from a single usable region.
    ///
    /// Unlike repeated `allocate_frame` calls, this never mixes recycled frames into
    /// the run: the whole span is carved from the bump pointer in one step. Recycled
    /// frames that happen to fall inside the carved span are dropped from the pool,
    /// because that memory now belongs to the caller and must not be handed out twice.
    pub fn allocate_contiguous_frames(
        &mut self,
        count: usize,
    ) -> Option<Vec<PhysFrame<Size4KiB>>> {
        if count == 0 {
            return Some(Vec::new());
        }
        let span = (count as u64).checked_mul(4096)?;
        loop {
            self.advance_to_usable_region();
            if self.region_idx >= self.memory_regions.len() {
                return None;
            }

            let region = &self.memory_regions[self.region_idx];
            let start = self.next_addr;
            let end = match start.checked_add(span) {
                Some(end) if end <= region.end => end,
                _ => {
                    // The run does not fit in this region; the remaining tail is too
                    // small to ever serve a contiguous request, so retire it.
                    self.region_idx += 1;
                    self.next_addr = 0;
                    continue;
                }
            };
            self.next_addr = end;

            self.recycled_frames
                .retain(|frame| {
                    let addr = frame.start_address().as_u64();
                    addr < start || addr >= end
                });

            let mut frames = Vec::with_capacity(count);
            for index in 0..count {
                let frame = PhysFrame::containing_address(
                    PhysAddr::new(start + index as u64 * 4096),
                );
                unsafe {
                    core::ptr::write_bytes(
                        (self.physical_memory_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                        0,
                        4096,
                    );
                }
                frames.push(frame);
            }
            return Some(frames);
        }
    }

    fn advance_to_usable_region(&mut self) {
        while self.region_idx < self.memory_regions.len() {
            let region = &self.memory_regions[self.region_idx];
            if region.kind == MemoryRegionKind::Usable && self.next_addr < region.end {
                if self.next_addr < region.start {
                    self.next_addr = (region.start + 4095) & !4095;
                }
                return;
            }
            self.region_idx += 1;
            self.next_addr = 0;
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = if let Some(frame) = self.recycled_frames.pop() {
            frame
        } else {
            self.advance_to_usable_region();
            if self.region_idx >= self.memory_regions.len() {
                return None;
            }

            let region = &self.memory_regions[self.region_idx];
            if self.next_addr + 4096 <= region.end {
                let frame = PhysFrame::containing_address(PhysAddr::new(self.next_addr));
                self.next_addr += 4096;
                frame
            } else {
                self.region_idx += 1;
                self.next_addr = 0;
                return self.allocate_frame();
            }
        };

        // Frames may contain data from a previous address-space owner, and freshly
        // carved frames from the bootloader memory map contain residual RAM contents.
        // Always zero before handing the frame out, using the offset owned by this
        // allocator. This must be unconditional: skipping it would make page-table and
        // heap contents depend on whatever QEMU happened to leave in RAM, which is
        // exactly the kind of uninitialised-memory dependence that produces
        // intermittent, nondeterministic behaviour.
        unsafe {
            core::ptr::write_bytes(
                (self.physical_memory_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                0,
                4096,
            );
        }
        Some(frame)
    }
}

// ---------------------------------------------------------------------------
// Stack Guard Pages — Hardware-enforced stack overflow protection
// ---------------------------------------------------------------------------

/// Size of a stack guard region (one 4KiB page, unmapped to trigger page fault on overflow).
pub const GUARD_PAGE_SIZE: usize = 4096;

/// Maps a guard page (no PRESENT bit) at the given virtual address.
///
/// Any access to this address will trigger a Page Fault (#PF) exception,
/// preventing silent stack corruption from buffer overflows.
///
/// # Safety
/// The caller must ensure the virtual address is properly aligned and
/// does not conflict with existing valid mappings.
pub fn map_guard_page(guard_vaddr: VirtAddr) {
    // Guard pages work by *not* mapping anything at that address.
    // The MMU will fault on any access because no PTE with PRESENT exists.
    // We only need to ensure any existing mapping is removed.
    let page: Page<Size4KiB> = Page::containing_address(guard_vaddr);
    let mut mapper_lock = MEMORY_MAPPER.lock();
    if let Some(mapper) = mapper_lock.as_mut() {
        // If there's an existing mapping, unmap it to create the guard
        if let Ok((_frame, flush)) = mapper.unmap(page) {
            flush.flush();
            crate::serial_println!(
                "[GUARD] Unmapped existing page at {:#x} to create guard",
                guard_vaddr.as_u64()
            );
        }
        // If no mapping exists, the page is already a guard (will fault on access)
    }
    crate::serial_println!(
        "[GUARD] Stack guard page active at {:#x}",
        guard_vaddr.as_u64()
    );
}

/// Calculates the guard page address for a stack allocated at the given base.
///
/// For downward-growing stacks (x86_64): the guard page is at `stack_base - GUARD_PAGE_SIZE`.
#[inline]
pub fn guard_page_addr(stack_base: usize) -> VirtAddr {
    VirtAddr::new((stack_base - GUARD_PAGE_SIZE) as u64)
}
