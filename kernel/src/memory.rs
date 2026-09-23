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
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

static PHYSICAL_MEMORY_OFFSET: Mutex<Option<VirtAddr>> = Mutex::new(None);
static MEMORY_MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

/// Initializes global memory management singletons: page mapper, physical offset, and frame allocator.
pub fn init_memory(physical_memory_offset: VirtAddr, memory_regions: &'static MemoryRegions) {
    let mapper = unsafe { init_page_table(physical_memory_offset) };
    let frame_allocator = unsafe { BootInfoFrameAllocator::init(memory_regions) };

    *PHYSICAL_MEMORY_OFFSET.lock() = Some(physical_memory_offset);
    *MEMORY_MAPPER.lock() = Some(mapper);
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);
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
#[derive(Debug)]
pub struct TaskPageTable {
    pub pml4_frame: PhysFrame<Size4KiB>,
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

            // Mirror higher-half kernel entries (256..512) from current kernel PML4
            let (kernel_pml4_frame, _) = Cr3::read();
            let kernel_pml4_ptr = (phys_offset + kernel_pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
            let kernel_pml4 = &*kernel_pml4_ptr;

            for i in 256..512 {
                new_pml4[i] = kernel_pml4[i].clone();
            }
        }

        Ok(Self { pml4_frame })
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

/// Maps a virtual page to a physical frame with user accessibility enabled at the leaf PTE level.
pub fn map_user_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let mut mapper_lock = MEMORY_MAPPER.lock();
    let mapper = mapper_lock.as_mut().ok_or("Mapper not initialized")?;
    let mut frame_alloc_lock = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_alloc_lock.as_mut().ok_or("Frame allocator not initialized")?;

    // Map virtual page to physical frame
    unsafe {
        match mapper.map_to(page, frame, flags, frame_allocator) {
            Ok(tlb) => tlb.flush(),
            Err(MapToError::PageAlreadyMapped(_)) => {
                mapper
                    .update_flags(page, flags)
                    .map_err(|_| "Failed to update page flags")?
                    .flush();
            }
            Err(_) => return Err("Failed to map page to physical frame"),
        }
    }

    Ok(())
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

