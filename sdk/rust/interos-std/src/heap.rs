// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign userspace heap allocator for InterOS.
//!
//! Provides an opt-in `#![no_std]` `GlobalAlloc` implementation backed by
//! anonymous kernel memory mappings (`sys_mem_map`), gated by `CapType::MemoryHeap`.
//! Employs an atomic spinlock and segregated free-list coalescing with arbitrary
//! SIMD alignment support (16-byte, 32-byte AVX, 64-byte AVX-512).

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

#[repr(C, align(16))]
struct BlockHeader {
    size: usize,
    is_free: bool,
    next: *mut BlockHeader,
}

const HEADER_SIZE: usize = core::mem::size_of::<BlockHeader>();
const MIN_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB allocation quantum

/// Sovereign InterOS heap allocator.
pub struct InterOsHeap {
    lock: AtomicBool,
    head: core::cell::UnsafeCell<*mut BlockHeader>,
}

unsafe impl Sync for InterOsHeap {}
unsafe impl Send for InterOsHeap {}

impl InterOsHeap {
    /// Creates an uninitialized heap allocator ready for use with `#[global_allocator]`.
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            head: core::cell::UnsafeCell::new(core::ptr::null_mut()),
        }
    }

    #[inline(always)]
    fn acquire(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    #[inline(always)]
    fn release(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

unsafe impl GlobalAlloc for InterOsHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.acquire();

        let align = layout.align().max(16);
        let needed_size = (layout.size().max(align) + (align - 1)) & !(align - 1);
        let head_ptr = self.head.get();
        let mut curr = *head_ptr;

        // 1. Search for existing suitable free block
        while !curr.is_null() {
            if (*curr).is_free {
                let raw_payload = curr as usize + HEADER_SIZE;
                let rem = raw_payload % align;
                let mut offset = if rem == 0 { 0 } else { align - rem };

                // If offset is smaller than HEADER_SIZE, placing an alignment BlockHeader
                // would overlap with curr's BlockHeader. Advance by align to ensure
                // offset >= HEADER_SIZE.
                if offset > 0 && offset < HEADER_SIZE {
                    offset += align;
                }

                if (*curr).size >= needed_size + offset {
                    // If alignment offset is needed, insert an alignment header
                    let active_block = if offset > 0 {
                        let rem_orig = (*curr).size - offset;
                        let aligned_ptr = (curr as *mut u8).add(offset) as *mut BlockHeader;
                        (*aligned_ptr).size = rem_orig;
                        (*aligned_ptr).is_free = true;
                        (*aligned_ptr).next = (*curr).next;

                        let prefix_size = offset - HEADER_SIZE;
                        (*curr).size = prefix_size;
                        (*curr).next = aligned_ptr;
                        aligned_ptr
                    } else {
                        curr
                    };

                    // Split block if remaining space is sufficient
                    if (*active_block).size >= needed_size + HEADER_SIZE + 16 {
                        let rem_size = (*active_block).size - needed_size - HEADER_SIZE;
                        let split_ptr = (active_block as *mut u8).add(HEADER_SIZE + needed_size) as *mut BlockHeader;
                        (*split_ptr).size = rem_size;
                        (*split_ptr).is_free = true;
                        (*split_ptr).next = (*active_block).next;

                        (*active_block).next = split_ptr;
                        (*active_block).size = needed_size;
                    }

                    (*active_block).is_free = false;
                    self.release();
                    return (active_block as *mut u8).add(HEADER_SIZE);
                }
            }
            curr = (*curr).next;
        }

        // 2. No suitable block found; map a new chunk from kernel
        let total_needed = needed_size + HEADER_SIZE + 2 * align;
        let chunk_size = total_needed.max(MIN_CHUNK_SIZE);
        let chunk_size_aligned = (chunk_size + 4095) & !4095;

        let map_res = crate::mem_map(
            0,
            chunk_size_aligned as u64,
            crate::PROT_READ | crate::PROT_WRITE,
            crate::MAP_ANONYMOUS | crate::MAP_PRIVATE,
        );

        let base = match map_res {
            Ok(ptr) => ptr,
            Err(_) => {
                self.release();
                return core::ptr::null_mut();
            }
        };

        let new_chunk = base as *mut BlockHeader;
        (*new_chunk).size = chunk_size_aligned - HEADER_SIZE;
        (*new_chunk).is_free = true;
        (*new_chunk).next = *head_ptr;
        *head_ptr = new_chunk;

        // Allocate from newly mapped chunk with lock held
        let raw_payload = new_chunk as usize + HEADER_SIZE;
        let rem = raw_payload % align;
        let mut offset = if rem == 0 { 0 } else { align - rem };
        if offset > 0 && offset < HEADER_SIZE {
            offset += align;
        }

        let active_block = if offset > 0 {
            let rem_orig = (*new_chunk).size - offset;
            let aligned_ptr = (new_chunk as *mut u8).add(offset) as *mut BlockHeader;
            (*aligned_ptr).size = rem_orig;
            (*aligned_ptr).is_free = true;
            (*aligned_ptr).next = (*new_chunk).next;

            let prefix_size = offset - HEADER_SIZE;
            (*new_chunk).size = prefix_size;
            (*new_chunk).next = aligned_ptr;
            aligned_ptr
        } else {
            new_chunk
        };

        if (*active_block).size >= needed_size + HEADER_SIZE + 16 {
            let rem_size = (*active_block).size - needed_size - HEADER_SIZE;
            let split_ptr = (active_block as *mut u8).add(HEADER_SIZE + needed_size) as *mut BlockHeader;
            (*split_ptr).size = rem_size;
            (*split_ptr).is_free = true;
            (*split_ptr).next = (*active_block).next;

            (*active_block).next = split_ptr;
            (*active_block).size = needed_size;
        }

        (*active_block).is_free = false;
        self.release();
        (active_block as *mut u8).add(HEADER_SIZE)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }

        self.acquire();

        let block = (ptr as usize - HEADER_SIZE) as *mut BlockHeader;
        (*block).is_free = true;

        // Coalesce physically adjacent free blocks
        let mut curr = *self.head.get();
        while !curr.is_null() {
            let next = (*curr).next;
            if !next.is_null() && (*curr).is_free && (*next).is_free {
                let expected_next = (curr as *mut u8).add(HEADER_SIZE + (*curr).size) as *mut BlockHeader;
                if next == expected_next {
                    (*curr).size += HEADER_SIZE + (*next).size;
                    (*curr).next = (*next).next;
                    continue; // Recheck current block with newly merged next
                }
            }
            curr = (*curr).next;
        }

        self.release();
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;

    #[test]
    fn test_interos_heap_layout_and_alignment() {
        assert_eq!(core::mem::align_of::<BlockHeader>(), 16);
        assert_eq!(HEADER_SIZE % 16, 0);
        let heap = InterOsHeap::new();
        assert!(!heap.lock.load(Ordering::Relaxed));
        assert!(unsafe { (*heap.head.get()).is_null() });
    }

    #[test]
    fn test_interos_heap_simd_alignment_and_coalescing() {
        assert_eq!(core::mem::align_of::<BlockHeader>(), 16);
        assert_eq!(HEADER_SIZE % 16, 0);

        let mut raw_buffer = alloc::vec![0u8; 65536 + 64];
        let raw_addr = raw_buffer.as_mut_ptr() as usize;
        let aligned_addr = (raw_addr + 63) & !63;
        let initial_chunk = aligned_addr as *mut BlockHeader;

        let heap = InterOsHeap::new();
        unsafe {
            (*initial_chunk).size = 65536 - HEADER_SIZE;
            (*initial_chunk).is_free = true;
            (*initial_chunk).next = core::ptr::null_mut();
            *heap.head.get() = initial_chunk;

            // Test 1: 32-byte alignment (AVX)
            let layout_avx = Layout::from_size_align(128, 32).unwrap();
            let ptr_avx = heap.alloc(layout_avx);
            assert!(!ptr_avx.is_null());
            assert_eq!(ptr_avx as usize % 32, 0, "AVX allocation must be 32-byte aligned");
            core::ptr::write_bytes(ptr_avx, 0xAA, 128);

            // Test 2: 64-byte alignment (AVX-512)
            let layout_avx512 = Layout::from_size_align(256, 64).unwrap();
            let ptr_avx512 = heap.alloc(layout_avx512);
            assert!(!ptr_avx512.is_null());
            assert_eq!(ptr_avx512 as usize % 64, 0, "AVX-512 allocation must be 64-byte aligned");
            core::ptr::write_bytes(ptr_avx512, 0xBB, 256);

            // Test 3: Free in reverse order and verify coalescing
            heap.dealloc(ptr_avx512, layout_avx512);
            heap.dealloc(ptr_avx, layout_avx);

            // After coalescing, head should be merged back into a single large free block
            let head = *heap.head.get();
            assert!(!head.is_null());
            assert_eq!(head, initial_chunk);
            assert_eq!((*head).size, 65536 - HEADER_SIZE, "Deallocation must coalesce back to full chunk size");
            assert!((*head).is_free);
        }
    }

    #[test]
    fn test_interos_heap_sub_header_alignment_offset_no_overlap() {
        let mut raw_buffer = alloc::vec![0u8; 65536 + 128];
        let raw_addr = raw_buffer.as_mut_ptr() as usize;
        let aligned_addr = (raw_addr + 127) & !127;
        let initial_chunk = aligned_addr as *mut BlockHeader;

        let heap = InterOsHeap::new();
        unsafe {
            (*initial_chunk).size = 65536 - HEADER_SIZE;
            (*initial_chunk).is_free = true;
            (*initial_chunk).next = core::ptr::null_mut();
            *heap.head.get() = initial_chunk;

            // First allocation of 16 bytes to offset the free payload
            let l1 = Layout::from_size_align(16, 16).unwrap();
            let p1 = heap.alloc(l1);
            assert!(!p1.is_null());

            // Next allocation requiring 64-byte alignment
            let l2 = Layout::from_size_align(64, 64).unwrap();
            let p2 = heap.alloc(l2);
            assert!(!p2.is_null());
            assert_eq!(p2 as usize % 64, 0);

            // Deallocate all and verify total coalescing
            heap.dealloc(p2, l2);
            heap.dealloc(p1, l1);

            let head = *heap.head.get();
            assert_eq!(head, initial_chunk);
            assert_eq!((*head).size, 65536 - HEADER_SIZE);
            assert!((*head).is_free);
        }
    }
}
