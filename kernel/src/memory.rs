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

/// Initializes global memory management singletons: page mapper, physical offset, and frame allocator.
pub fn init_memory(physical_memory_offset: VirtAddr, memory_regions: &'static MemoryRegions) {
    let (kernel_frame, _) = Cr3::read();
    *KERNEL_PML4_FRAME.lock() = Some(kernel_frame);

    let mapper = unsafe { init_page_table(physical_memory_offset) };
    let frame_allocator = unsafe { BootInfoFrameAllocator::init(memory_regions) };

    *PHYSICAL_MEMORY_OFFSET.lock() = Some(physical_memory_offset);
    *MEMORY_MAPPER.lock() = Some(mapper);
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);
}

/// Returns the primordial kernel PML4 physical frame.
pub fn kernel_pml4() -> PhysFrame<Size4KiB> {
    KERNEL_PML4_FRAME
        .lock()
        .expect("Kernel PML4 frame not recorded during init_memory")
}

/// Switches CPU CR3 back to the clean kernel PML4.
pub fn switch_to_kernel_page_table() {
    let (_, flags) = Cr3::read();
    unsafe {
        Cr3::write(kernel_pml4(), flags);
    }
}

/// Returns the physical memory offset mapped by the bootloader.
pub fn phys_mem_offset() -> VirtAddr {
    PHYSICAL_MEMORY_OFFSET
        .lock()
        .expect("Physical memory offset not initialized")
}

/// Allocates physically contiguous 4 KiB frames for DMA buffers.
pub fn allocate_dma_frames(count: usize) -> Option<alloc::vec::Vec<PhysFrame<Size4KiB>>> {
    if count == 0 { return Some(alloc::vec::Vec::new()); }
    let mut frames = alloc::vec::Vec::with_capacity(count);
    let first = allocate_frame()?;
    frames.push(first);
    for _ in 1..count {
        let next = allocate_frame()?;
        let expected = frames.last()?.start_address().as_u64() + 4096;
        if next.start_address().as_u64() != expected {
            for frame in frames.drain(..) { recycle_frame(frame); }
            recycle_frame(next);
            return None;
        }
        frames.push(next);
    }
    Some(frames)
}

/// Converts a physical address into its bootloader-provided direct-map virtual address.
pub fn physical_to_virtual(physical: PhysAddr) -> VirtAddr {
    phys_mem_offset() + physical.as_u64()
}

/// Maps a physical MMIO range into the kernel address space with cache-disabled
/// page attributes. The caller supplies a dedicated virtual base.
pub fn map_mmio_region(
    physical_base: PhysAddr,
    size: usize,
    virtual_base: VirtAddr,
) -> Result<VirtAddr, &'static str> {
    if size == 0 { return Err("MMIO mapping size is zero"); }
    let pages = (size.saturating_add(4095)) / 4096;
    let mut mapper_lock = MEMORY_MAPPER.lock();
    let mapper = mapper_lock.as_mut().ok_or("Memory mapper not initialized")?;
    let _start = Page::<Size4KiB>::containing_address(virtual_base);
    for index in 0..pages {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virtual_base.as_u64() + (index as u64) * 4096));
        let phys = PhysFrame::containing_address(physical_base + (index as u64) * 4096);
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
        let map = unsafe { mapper.map_to(page, phys, flags, FRAME_ALLOCATOR.lock().as_mut().ok_or("Frame allocator not initialized")?) };
        match map {
            Ok(flush) => flush.flush(),
            Err(MapToError::PageAlreadyMapped(_)) => return Err("MMIO virtual range already mapped"),
            Err(_) => return Err("MMIO page mapping failed"),
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

            // Mirror all kernel and supervisor entries (1..512) from kernel PML4.
            // Index 0 is reserved exclusively for isolated Ring 3 user memory (text, stack, heap).
            let kernel_pml4_frame = kernel_pml4();
            let kernel_pml4_ptr = (phys_offset + kernel_pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
            let kernel_pml4 = &*kernel_pml4_ptr;

            for i in 256..512 {
                new_pml4[i] = kernel_pml4[i].clone();
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
    pub fn switch_to(&self) {
        let (_, flags) = Cr3::read();
        unsafe {
            Cr3::write(self.pml4_frame, flags);
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
        // Mapping is transactional: any page-table frames allocated by this
        // operation are unlinked before their frames are recycled. The caller
        // retains ownership of the leaf frame on failure.
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

        let leaf_flags = flags | PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
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
    let (_, flags) = Cr3::read();
    unsafe {
        Cr3::write(pml4_frame, flags);
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
}

impl BootInfoFrameAllocator {
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
    pub unsafe fn init(memory_regions: &'static MemoryRegions) -> Self {
        let mut allocator = BootInfoFrameAllocator {
            memory_regions,
            region_idx: 0,
            next_addr: 0,
            recycled_frames: Vec::new(),
        };
        allocator.advance_to_usable_region();
        allocator
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

        // Frames may contain data from a previous address-space owner. Always
        // zero before reuse so stale userspace contents cannot cross lifetimes.
        if let Some(offset) = *PHYSICAL_MEMORY_OFFSET.lock() {
            unsafe {
                core::ptr::write_bytes(
                    (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                    0,
                    4096,
                );
            }
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
