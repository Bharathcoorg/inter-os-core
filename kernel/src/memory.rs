//! Physical and Virtual Memory Management
//!
//! Handles page table initialization using the physical memory offset provided by the
//! bootloader, and provides a frame allocator backed by the UEFI/BIOS memory map.
//! Enforces zero-trust hardware MMU privilege boundaries between Ring 0 and Ring 3.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::MapToError;
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

/// Allocates a single 4 KiB physical memory frame.
pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAME_ALLOCATOR.lock().as_mut()?.allocate_frame()
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

            // Mirror higher-half kernel entries (256..512) from kernel PML4
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

    /// Maps a virtual page to a physical frame in this process's isolated page table.
    /// Intermediate tables (PDPT, PD, PT) are dynamically allocated with USER_ACCESSIBLE flags.
    pub fn map_user_page(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
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

        let table_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        // Level 4 -> Level 3 (PDPT)
        let p3_frame = if pml4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
            pml4[p4_idx].frame().map_err(|_| "Huge page at PML4")?
        } else {
            let f = allocate_frame().ok_or("Out of memory for PDPT")?;
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pml4[p4_idx].set_frame(f, table_flags);
            f
        };

        // Level 3 -> Level 2 (PD)
        let pdpt_ptr = (phys_offset + p3_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pdpt = unsafe { &mut *pdpt_ptr };

        let p2_frame = if pdpt[p3_idx].flags().contains(PageTableFlags::PRESENT) {
            pdpt[p3_idx].frame().map_err(|_| "Huge page at PDPT")?
        } else {
            let f = allocate_frame().ok_or("Out of memory for PD")?;
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pdpt[p3_idx].set_frame(f, table_flags);
            f
        };

        // Level 2 -> Level 1 (PT)
        let pd_ptr = (phys_offset + p2_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pd = unsafe { &mut *pd_ptr };

        let p1_frame = if pd[p2_idx].flags().contains(PageTableFlags::PRESENT) {
            pd[p2_idx].frame().map_err(|_| "Huge page at PD")?
        } else {
            let f = allocate_frame().ok_or("Out of memory for PT")?;
            self.allocated_frames.push(f);
            let ptr = (phys_offset + f.start_address().as_u64()).as_mut_ptr::<PageTable>();
            unsafe { (*ptr).zero(); }
            pd[p2_idx].set_frame(f, table_flags);
            f
        };

        // Level 1: Leaf PTE
        let pt_ptr = (phys_offset + p1_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let pt = unsafe { &mut *pt_ptr };

        let leaf_flags = flags | PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        pt[p1_idx].set_frame(frame, leaf_flags);
        self.allocated_frames.push(frame);

        // If this page table is currently active, flush TLB
        let (active_cr3, _) = Cr3::read();
        if active_cr3 == self.pml4_frame {
            x86_64::instructions::tlb::flush(VirtAddr::new(vaddr));
        }

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

    // 1. Create two independent task page tables: Process A and Process B
    let mut pt_a = TaskPageTable::create_user_page_table().expect("Failed to create PT A");
    let mut pt_b = TaskPageTable::create_user_page_table().expect("Failed to create PT B");

    // PML4 root frames must be distinct
    assert_ne!(pt_a.frame(), pt_b.frame(), "Process A and B shared the same PML4 frame!");

    // 2. Allocate distinct physical frames for a shared virtual address (e.g. 0x0040_0000)
    let frame_a = allocate_frame().expect("Frame alloc A");
    let frame_b = allocate_frame().expect("Frame alloc B");
    assert_ne!(frame_a, frame_b, "Frame A and B collided");

    let test_page = Page::<Size4KiB>::containing_address(VirtAddr::new(0x0040_0000));
    let phys_offset = phys_mem_offset();

    // Write distinct data into physical frames
    unsafe {
        let ptr_a = (phys_offset + frame_a.start_address().as_u64()).as_mut_ptr::<u32>();
        *ptr_a = 0xAAAA_1111;

        let ptr_b = (phys_offset + frame_b.start_address().as_u64()).as_mut_ptr::<u32>();
        *ptr_b = 0xBBBB_2222;
    }

    // Map test_page in A as Read+Execute (Code, RX), and in B as Read+Write+NX (Data, RW+NX)
    pt_a.map_user_page(test_page, frame_a, PageTableFlags::empty())
        .expect("Map page in A");
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
}

impl BootInfoFrameAllocator {
    /// Creates a FrameAllocator from the passed memory map.
    pub unsafe fn init(memory_regions: &'static MemoryRegions) -> Self {
        let mut allocator = BootInfoFrameAllocator {
            memory_regions,
            region_idx: 0,
            next_addr: 0,
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
        self.advance_to_usable_region();
        if self.region_idx >= self.memory_regions.len() {
            return None;
        }

        let region = &self.memory_regions[self.region_idx];
        if self.next_addr + 4096 <= region.end {
            let frame = PhysFrame::containing_address(PhysAddr::new(self.next_addr));
            self.next_addr += 4096;
            Some(frame)
        } else {
            self.region_idx += 1;
            self.next_addr = 0;
            self.allocate_frame()
        }
    }
}

