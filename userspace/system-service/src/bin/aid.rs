#![no_std]
#![no_main]

// The heap the inference engine cuts its scratch and KV cache from is declared
// once, in this crate's library (`src/lib.rs`), over
// `interos_std::heap::InterOsHeap`: a `#[global_allocator]` applies to every
// binary that links an allocating library, so it cannot be spelled only in this
// file. What makes it `aid`-only in effect is the kernel: `aid` is the one
// service granted `CapType::MemoryHeap` (`kernel/src/services.rs`), so only its
// mappings are honoured.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    interos_system_service::run_ai();
}
