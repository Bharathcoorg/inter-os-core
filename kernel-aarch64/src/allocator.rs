//! AArch64 Kernel Heap Allocator
//!
//! Provides bare-metal dynamic allocation (`Box`, `Vec`, `String`)
//! using a thread-safe `LockedHeap` linked-list allocator.

use linked_list_allocator::LockedHeap;

pub const HEAP_SIZE: usize = 8 * 1024 * 1024; // 8 MiB Kernel Heap

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

static mut HEAP_SPACE: [u8; HEAP_SIZE] = [0u8; HEAP_SIZE];

pub fn init_heap() {
    unsafe {
        let heap_ptr = core::ptr::addr_of_mut!(HEAP_SPACE) as *mut u8;
        ALLOCATOR.lock().init(heap_ptr, HEAP_SIZE);
    }
}
