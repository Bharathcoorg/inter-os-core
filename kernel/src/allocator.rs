//! Kernel Heap Allocator
//!
//! Maps virtual memory pages for the kernel heap and initializes the linked-list
//! allocator, enabling dynamic heap allocations (`Box`, `Vec`, `String`, etc.)
//! in `no_std` kernel mode.

#[cfg(not(test))]
use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

/// Virtual address where the kernel heap begins.
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Initial size of the kernel heap (64 MiB).
pub const HEAP_SIZE: usize = 64 * 1024 * 1024;

// WHY: the bare-metal heap is `HEAP_START..HEAP_START + HEAP_SIZE` of pages that
// `init_heap` maps before the heap is ever locked. A host unit-test binary has
// no such pages, so `LockedHeap::empty()` aborted the process on its first
// allocation -- which is why every test in this crate used to die before running.
// Under `cfg(test)` we therefore delegate to the host allocator instead, so
// `Box`/`Vec`/`String` work in tests without pretending a kernel heap exists.
#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: HostAllocator = HostAllocator;

/// Host-backed global allocator, used only by `cargo test`.
#[cfg(test)]
struct HostAllocator;

#[cfg(test)]
unsafe impl core::alloc::GlobalAlloc for HostAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        std::alloc::System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        std::alloc::System.dealloc(ptr, layout)
    }
}

/// Initializes the kernel heap by mapping pages and initializing the locked heap allocator.
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() };
    }

    // WHY: a host test binary has no pages at HEAP_START and no LockedHeap to
    // lock. The mapping above still compiles but is unreachable, because only
    // the boot path in `memory.rs` calls `init_heap`; skipping the allocator
    // init here would otherwise silently look like a working kernel heap.
    #[cfg(not(test))]
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}

/// Returns (used_bytes, free_bytes) from the locked kernel heap.
#[cfg(not(test))]
pub fn heap_stats() -> (usize, usize) {
    let heap = ALLOCATOR.lock();
    (heap.used(), heap.free())
}

/// Host test builds have no kernel heap, so this refuses rather than reporting
/// invented numbers.
///
/// WHY: the `[DIAG_STATE]` serial line is a test-harness contract literal and
/// cannot be reworded to say "unavailable" without breaking the harnesses that
/// parse it. Returning `0, 0` here would be a fabricated measurement of a heap
/// that does not exist, so the only honest option left is to fail loudly. Only
/// the boot-time diagnostics in `main.rs` and `serial.rs` call this, so no unit
/// test reaches it.
#[cfg(test)]
pub fn heap_stats() -> (usize, usize) {
    panic!("heap_stats() is meaningless outside the bare-metal kernel heap");
}
