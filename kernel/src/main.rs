// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Kernel Entry Point
//!
//! Initializes low-level architecture subsystems (GDT, TSS, IDT, PIC, PIT Timer,
//! Serial COM1, Framebuffer Console, Physical Paging, and Heap Allocator),
//! brings up the ATA PIO Block Driver, SHA-256 Cryptographic Subsystem,
//! dm-verity Merkle Tree Verifier, Capability-Governed VFS, local Capability Tool Engine,
//! and pure `no_std` WebAssembly (WASM) Sandboxed Runtime.
//!
//! The modules themselves live in the `kernel` library so they can be unit
//! tested on the host; this binary only owns the panic handler and the boot
//! sequence.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use bootloader_api::config::Mapping;
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use x86_64::VirtAddr;

use kernel::capability::{CapError, Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use kernel::crypto::Sha256;
use kernel::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
use kernel::ipc::IPC_ROUTER;
use kernel::package::RappArch;
use kernel::storage::{MerkleVerifier, StorageError};
use kernel::task::{SCHEDULER, TaskId};
use kernel::wasm::{opcodes, Value, WasmError, WasmMemory, WasmModule, WasmVm};

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.kernel_stack_size = 512 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Primary kernel entry function invoked by the bootloader.
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    x86_64::instructions::interrupts::disable();

    // 1. Initialize COM1 serial port for diagnostics and test logging
    kernel::serial::init();
    serial_println!("[BOOT] Serial COM1 online. Checking relocations...");

    // 1b. Apply early dynamic relocations if kernel was loaded at higher-half offset
    let num_relocated = unsafe { kernel::arch::apply_relocations() };
    serial_println!("[BOOT] Relocations applied: {}", num_relocated);

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        kernel::framebuffer::init(framebuffer);
    }

    if num_relocated > 0 {
        serial_println!("[OK] Dynamic self-relocations applied: {} symbols patched.", num_relocated);
    }

    // 2. Output diagnostic banner to UART serial COM1
    serial_println!("================================================================================");
    serial_println!("  INTER OS CORE -- Open-Source Rust Mobile Microkernel (x86_64)");
    serial_println!("  Security: Zero-Trust Capability Microkernel | Mode: Ring 0/Ring 3 User Separation");
    serial_println!("  Subsystem: Hardware Drivers, Memory Management, IPC, VFS & Capabilities");
    serial_println!("================================================================================");

    // 3. Initialize Global Descriptor Table (GDT) and Task State Segment (TSS)
    kernel::gdt::init();
    serial_println!("[OK] GDT & TSS loaded with dedicated Double Fault IST stack & Ring 3 segments.");

    // 4. Initialize Interrupt Descriptor Table (IDT) for exceptions and hardware IRQs
    kernel::interrupts::init_idt();
    serial_println!("[OK] IDT loaded with CPU exception and hardware interrupt gates.");

    // 5. Initialize 8259 PIC controllers (PIT timer deferred until scheduler start)
    kernel::interrupts::init_pics();
    serial_println!("[OK] Dual 8259 PIC remapped (IRQs 32-47).");
    if !kernel::drivers::ps2_mouse::init() {
        serial_println!("[INPUT] PS/2 mouse unavailable; continuing with other configured input transports.");
    }

    // 6. Initialize virtual memory page tables and physical frame allocator
    let phys_mem_offset_raw = boot_info
        .physical_memory_offset
        .into_option()
        .expect("Bootloader failed to map physical memory offset");
    let phys_mem_offset = VirtAddr::new(phys_mem_offset_raw);

    kernel::memory::init_memory(phys_mem_offset, &boot_info.memory_regions);
    serial_println!("[OK] Page tables and physical frame allocator initialized.");

    // 7. Initialize kernel heap allocator
    kernel::memory::init_kernel_heap()
        .expect("Kernel heap initialization failed");
    serial_println!("[OK] Kernel 64 MiB dynamic heap allocator online.");

    // Engage the native 720x1440 DISPI scanout now that BOTH the kernel
    // page-table mapper and the heap are live: the framebuffer's mapped tail
    // is extended first (map_mmio_region tracks its pages in a heap Vec),
    // then (only if provably safe) the physical mode is reprogrammed.
    // Deferred from framebuffer::init because reprogramming before paging
    // existed wrote past mapped pages and trapped the guest in a reset loop;
    // placing it after heap init because the mapping step allocates.
    kernel::framebuffer::engage_native_mode();

    // Run Hardware MMU & Memory Isolation Adversarial Tests
    kernel::memory::run_mmu_adversarial_tests();

    // Establish the truthful runtime/platform contract before subsystem self-tests.
    kernel::platform::init();

    // 8. Run kernel core self-tests
    serial_println!("\n--- [Running Kernel Core Self-Tests] ---");

    // Test 8a: Dynamic Box allocation
    let test_box = Box::new(0xCAFE_BABE_u64);
    assert_eq!(*test_box, 0xCAFE_BABE_u64);
    serial_println!("[TEST] Box dynamic heap verified: {:#X}", *test_box);

    // Test 8b: Dynamic Vec reallocation
    let mut test_vec = Vec::new();
    for i in 0..500 {
        test_vec.push(i);
    }
    assert_eq!(test_vec.len(), 500);
    serial_println!("[TEST] Vec dynamic heap verified: length = 500");

    // Note: this used to execute a bare `int3` here to assert that the debug
    // exception handler "recovered cleanly". Triggering a debug exception during
    // kernel initialisation provides no security or functional guarantee, and it
    // meant any fault in the handler or the code following it halted the whole
    // device before userspace existed. The breakpoint handler stays installed so
    // a real debugger breakpoint is still handled; its behaviour is covered by
    // unit tests rather than by a live trap in the boot path.

    // Test 8d: Pure no_std SHA-256 Cryptographic Engine Verification
    let sample_text = b"Inter OS Sovereign Post-App Operating System";
    let hash1 = Sha256::digest(sample_text);
    let hash2 = Sha256::digest(sample_text);
    assert_eq!(hash1, hash2);
    assert_ne!(hash1, [0u8; 32]);
    serial_println!("[TEST] SHA-256 cryptographic digest verified: {:02X}{:02X}{:02X}{:02X}...",
        hash1[0], hash1[1], hash1[2], hash1[3]);

    // Test 8e: Cryptographic Merkle Block Verification (dm-verity)
    let block_data = b"Dm-verity verified system partition block #0001";
    let expected_hash = Sha256::digest(block_data);
    let verify_pass = MerkleVerifier::verify_block(block_data, &expected_hash);
    assert!(verify_pass.is_ok());

    let corrupted_block = b"Dm-verity TAMPERED system partition block #0001";
    let verify_tamper = MerkleVerifier::verify_block(corrupted_block, &expected_hash);
    assert_eq!(verify_tamper, Err(StorageError::IntegrityViolation));
    serial_println!("[TEST] Merkle integrity attestation: VALIDATED (Tampering rejected).");

    // Initialize x86_64 Fast System Call MSRs (IA32_EFER.SCE, STAR, LSTAR, FMASK)
    kernel::syscall::entry::init();
    let dispatch_tid = kernel::syscall::sys_dispatch(7, 0, 0, 0, 0, 0);
    assert_eq!(dispatch_tid, 0, "Syscall dispatcher must return Task 0 during boot");
    serial_println!("[TEST] Hardware Fast Syscall Subsystem: MSRs programmed, dispatcher verified TaskId(0).");

    // Test 8f: WebAssembly Stack VM Arithmetic Self-Test
    let mut wasm_test_module = WasmModule::new();
    let mut compute_code = Vec::new();
    compute_code.push(opcodes::LOCAL_GET);
    compute_code.push(0); // param 0
    compute_code.push(opcodes::LOCAL_GET);
    compute_code.push(1); // param 1
    compute_code.push(opcodes::I32_ADD);
    compute_code.push(opcodes::I32_CONST);
    compute_code.push(10); // * 10
    compute_code.push(opcodes::I32_MUL);
    compute_code.push(opcodes::RETURN);
    compute_code.push(opcodes::END);

    let fn_idx = wasm_test_module.add_function("compute", 2, 0, compute_code);
    wasm_test_module.add_export("compute", fn_idx);
    let test_mem = WasmMemory::new(1, 1);
    let mut test_vm = WasmVm::new(wasm_test_module, test_mem, 100);
    let vm_res = test_vm.invoke_export("compute", &[Value::I32(5), Value::I32(7)]);
    assert_eq!(vm_res, Ok(Some(Value::I32(120))));
    serial_println!("[TEST] WASM Stack VM arithmetic execution verified: (5 + 7) * 10 = 120 (Instructions: {}).", test_vm.instructions_executed);

    // Test 8g: WASM Linear Memory Sandbox Protection
    let mut bounds_mem = WasmMemory::new(1, 1); // 64 KiB
    assert!(bounds_mem.write_u8(100, 0xAA).is_ok());
    assert_eq!(bounds_mem.write_u8(70_000, 0xFF), Err(WasmError::OutOfBoundsMemory));
    serial_println!("[TEST] WASM Sandbox linear memory protection: Out-of-bounds access trapped.");

    // Test 8h: WASM Fuel Quota & DoS Protection
    let mut loop_module = WasmModule::new();
    let mut inf_code = Vec::new();
    for _ in 0..10 {
        inf_code.push(opcodes::NOP);
    }
    inf_code.push(opcodes::END);
    let inf_fn = loop_module.add_function("loop", 0, 0, inf_code);
    loop_module.add_export("loop", inf_fn);
    let loop_mem = WasmMemory::new(1, 1);
    let mut loop_vm = WasmVm::new(loop_module, loop_mem, 3); // only 3 fuel units
    assert_eq!(loop_vm.invoke_export("loop", &[]), Err(WasmError::FuelExhausted));
    serial_println!("[TEST] WASM Fuel quota protection verified: Execution halted on FuelExhausted.");

    // 9. Primary ATA / IDE PIO Block Driver Initialization
    serial_println!("\n--- [Initializing Hardware Storage Block Drivers] ---");
    match kernel::drivers::ata::init() {
        Ok(()) => {
            let mut ata_guard = ATA_PRIMARY.lock();
            if let Some(drive) = ata_guard.as_mut() {
                let model = drive.model();
                let sectors = drive.sector_count();
                serial_println!("[OK] Primary ATA Controller online. Model: '{}' | Sectors: {}", model, sectors);

                // Live hardware sector read test on LBA 0 (MBR sector)
                let mut mbr_buf = [0u8; SECTOR_SIZE];
                match drive.read_sector(0, &mut mbr_buf) {
                    Ok(()) => {
                        let b510 = mbr_buf[510];
                        let b511 = mbr_buf[511];
                        serial_println!("[OK] Real QEMU Hardware Disk Sector 0 read successfully. Magic signature: {:#04X} {:#04X}", b510, b511);
                        if b510 == 0x55 && b511 == 0xAA {
                            serial_println!("[OK] Hardware Boot Signature 0x55AA VERIFIED on raw disk.");
                        }
                    }
                    Err(e) => {
                        serial_println!("[WARN] Failed to read LBA 0: {:?}", e);
                    }
                }
            }
        }
        Err(e) => {
            serial_println!("[WARN] ATA drive initialization skipped or not present: {:?}", e);
        }
    }

    // 9b. Modular Hardware Driver Subsystems (DRM/KMS, FDT, PMIC, Touch)
    kernel::drivers::init_and_test_drivers();

    // Establish the persistent device key before any VFS payload is reconstructed.
    // The seed is loaded/generated after the physical block device is online.
    kernel::crypto::rng::init();
    kernel::crypto::keystore::init();
    serial_println!("[OK] Persistent storage encryption root initialized before VFS mount.");

    // 10. Mount the canonical block-backed filesystem before VFS reconstruction.
    match kernel::storage::init_block_filesystem() {
        Ok(()) => serial_println!("[OK] Persistent InterFS region mounted/formatted."),
        Err(e) => serial_println!("[WARN] Persistent InterFS unavailable: {:?}", e),
    }

    // 10b. Virtual Filesystem (VFS) Initialization & Package Runtime
    serial_println!("\n--- [Initializing Virtual Filesystem & Package Runtime] ---");
    // Persistent storage is now ready; recovery/update journals are restored before userspace starts.
    kernel::recovery::init();
    kernel::update::init();

    kernel::fs::init();
    kernel::package::PACKAGE_BLOB_STORE.lock().restore();
    kernel::package::PACKAGE_REGISTRY.lock().seed();
    kernel::package::REVOCATION_REGISTRY.lock().seed();
    // WHY the intent registry is seeded here: a package's declared handlers
    // are part of what the device is, so they have to exist before the first
    // app asks "who handles this file?". The first-party applications run as
    // kernel-resident images rather than as installed RAPPs, so their
    // declarations are seeded from the table that mirrors their manifests.
    kernel::content::init();
    serial_println!("[PACKAGE] persistent registry, payload store and revocation state restored.");
    // WHY this is said at boot: the TLS trust anchors live in RAM, so the operator
    // who installed one yesterday cannot tell from the screen whether this boot
    // still holds it. Saying it once here -- before the first connection is
    // attempted -- is what turns "HTTPS failed" into "this device trusts nothing".
    kernel::crypto::tls_trust::report_at_boot();

    // Generate sample sandboxed package bytecode: returns 0 (exit success)
    let mut wasm_app_code = Vec::new();
    wasm_app_code.push(opcodes::I32_CONST);
    wasm_app_code.push(0); // return code 0 (success)
    wasm_app_code.push(opcodes::RETURN);
    wasm_app_code.push(opcodes::END);

    // RAPP v2 is fail-closed: unsigned kernel-built packages are never staged for execution.
    // Signed packages are produced by tools/interos.py and must be provisioned into the trust store.
    let _ = (RappArch::X86_64, wasm_app_code);
    serial_println!("[OK] RAPP v2 runtime ready: unsigned packages are rejected by the kernel trust boundary.");
    serial_println!("[OK] Root filesystem mounted. RAPP v2 package staging is provisioned through the trusted package pipeline.");

    // 11. Zero-Trust Capability Enforcement Self-Tests
    serial_println!("\n--- [Zero-Trust Capability Enforcement Self-Tests] ---");
    let mut test_caps = CapabilityTable::new(8);
    let cap_log_idx = test_caps
        .insert(Capability {
            id: CapabilityId(1),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .expect("Failed to insert capability");

    // Verify authorized access
    let verify_ok = test_caps.verify(cap_log_idx, CapabilityType::SerialOutput, Rights::WRITE);
    assert!(verify_ok.is_ok());
    serial_println!("[TEST] Authorized capability check: GRANTED (SerialOutput | WRITE).");

    // Verify unauthorized rights escalation rejection
    let verify_no_exec = test_caps.verify(cap_log_idx, CapabilityType::SerialOutput, Rights::EXECUTE);
    assert_eq!(verify_no_exec, Err(CapError::InsufficientRights));
    serial_println!("[TEST] Unauthorized rights escalation: REJECTED (InsufficientRights).");

    // Verify unauthorized type mismatch rejection
    let verify_type_mismatch = test_caps.verify(cap_log_idx, CapabilityType::HardwarePortIo, Rights::READ);
    assert_eq!(verify_type_mismatch, Err(CapError::TypeMismatch));
    serial_println!("[TEST] Unauthorized resource access: REJECTED (TypeMismatch).");

    // Verify VFS file capability check
    let vfs_unauthorized = test_caps.verify(cap_log_idx, CapabilityType::FileSystemRead, Rights::READ);
    assert_eq!(vfs_unauthorized, Err(CapError::TypeMismatch));
    serial_println!("[TEST] Ambient VFS file access: REJECTED (TypeMismatch - Zero Ambient Authority).");
    serial_println!("--- [Capability Subsystem 100% Attested & Enforced] ---\n");

    // 12. Initialize Android Compatibility Runtime (Zone 4 Isolation)
    kernel::compat::android::init();



    // 15. Initialize persistent resource/recovery/update state after storage is mounted.
    kernel::resource::init();
    kernel::diagnostics::init();
    kernel::performance::init();

    // 16. Initialize Hardware Input Subsystem (Capacitive Touch Digitizer & Mobile Gesture Engine)
    kernel::input::init();

    // 19. Initialize Native Graphical User Shell & Window Compositor
    kernel::gui::init();

    // 20. Initialize PCI Bus Enumeration (Physical Peripheral Discovery)
    kernel::drivers::pci::init();

    // 21. Initialize In-Kernel Zero-Allocation Network Stack
    kernel::net::init();



    // 24. Initialize Multi-Architecture Mobile SoC Hardware Abstraction Layer (HAL)
    kernel::hal::init();

    // 24b. Initialize Multi-Architecture Hardware & Boot Image Layer (Fastboot v2 & AArch64)
    kernel::arch::init();

    // 25. Validate the already-initialized hardware entropy and secure keystore.
    let mut trng_test_buf = [0u8; 32];
    kernel::crypto::get_random_bytes(&mut trng_test_buf);
    assert_ne!(trng_test_buf, [0u8; 32], "Hardware TRNG must produce non-zero entropy");
    serial_println!("[TEST] Hardware TRNG: Generated 32 bytes entropy, non-zero randomness validated.");
    serial_println!("[OK] Hardware-isolated Secure Enclave & Keystore remains initialized.");

    // 27. Initialize Bare-Metal Mobile Audio Subsystem (PC Speaker & PCI Audio)
    kernel::audio::init();

    // 28. Initialize Sovereign Telephony & Cellular Baseband Modem Subsystem
    kernel::telephony::init();

    // 29. Initialize Mobile Hardware Sensor Subsystem (IMU, Orientation, Proximity)
    kernel::sensor::init();

    // 30. Initialize Storage Subsystems (Crash Dump & Encrypted Vault)
    kernel::storage::init();

    // 31. Initialize Cryptographic Secure IPC Channels
    kernel::ipc::init();

    // 31b. Initialize Asynchronous Event Notification Subsystem (Sovereign epoll/kqueue)
    kernel::event::init();

    // 31c. Verify .rapp package format round-trip (build → parse → SHA-256 verify)
    {
        use kernel::package::rapp::{parse_rapp, RappArch};
        let test_elf = b"\x7fELF_TEST_PAYLOAD_STUB";
        // Negative trust-boundary test: a zero-signature package must not parse.
        let rapp_bin = {
            let mut b = Vec::new();
            b.extend_from_slice(b"RAPP");
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(&(RappArch::X86_64 as u16).to_le_bytes());
            b.extend_from_slice(b"com.test.hello");
            b.resize(40, 0);
            b.extend_from_slice(&[0u8; 32]);
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(&kernel::crypto::Sha256::digest(test_elf));
            b.extend_from_slice(&[0u8; 64]);
            b.extend_from_slice(&(test_elf.len() as u32).to_le_bytes());
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(test_elf);
            b
        };
        assert!(parse_rapp(&rapp_bin).is_err());
        serial_println!("[TEST] RAPP v2 fail-closed unsigned-package rejection: PASSED");
    }

    // 32. Initialize Hardware MSR Fast System Call Extensions (SCE, STAR, LSTAR, FMASK)
    kernel::syscall::entry::init();

    // 32b. Initialize Zero-Trust Capability Delegation Subsystem
    kernel::capability::init();

    // 33. Initialize Multitasking Scheduler and Spawn System Tasks
    serial_println!("--- [Initializing Multitasking Scheduler & Spawning Tasks] ---");
    let mut main_caps = CapabilityTable::new(16);
    main_caps
        .insert(Capability {
            id: CapabilityId(10),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    SCHEDULER.lock().init(main_caps);
    serial_println!("[OK] Primordial Kernel Thread registered as Task 0.");

    // The always-on service set is a device-side choice read off the data disk,
    // so a tiered measurement boots one kernel image for every tier.
    kernel::services::load_boot_profile_from_device();

    // Kernel registers service definitions only. Ring-3 PID 1
    // owns service startup, recovery and lifecycle policy.
    kernel::services::init();
    kernel::settings::init();
    kernel::user::init();
    kernel::notification::init();

    // Primordial userspace init/service-manager. It owns service lifecycle
    // through the object-scoped ServiceStart/Stop/Recover ABI.
    let id_user_init = kernel::task::spawn_user_init(TaskId(0));
    IPC_ROUTER.lock().register_task(id_user_init);

    serial_println!("[OK] PID1 userspace service manager spawned (Task {}).", id_user_init.0);
    serial_println!("[OK] Kernel service registry initialized; application startup delegated to userspace.");

    let clock = kernel::interrupts::init_timekeeping();
    serial_println!("[OK] System clock source: {}.", clock);
    kernel::interrupts::init_pit();
    serial_println!("[OK] PIT system timer configured at {} Hz.", kernel::interrupts::SYSTEM_TICK_HZ);

    x86_64::instructions::interrupts::enable();
    serial_println!("[OK] Hardware interrupts enabled on CPU.");

    loop {
        // WHY serial commands are serviced here and not in the COM1 interrupt
        // handler: dispatching a TAP launches apps, renders surfaces and prints,
        // which can block on locks the interrupted code already holds and overruns
        // the 16-byte UART FIFO from interrupt context. The handler only fills the
        // line buffer; the work happens here, in a normal context.
        kernel::serial::service_serial_commands();
        kernel::services::supervise();
        // The process table is printed here, in a context where its bytes do not
        // hold the timer or a touch interrupt off -- and only when the task table
        // has changed shape since the last one the console was shown.
        kernel::interrupts::service_deferred_diagnostics();
        // Reclaim terminated tasks here, never from inside the scheduler lock:
        // the hooks take VFS, whose own paths take the scheduler lock.
        kernel::task::scheduler::run_deferred_cleanup();
        if !kernel::task::yield_now() {
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }
}

// ---------------------------------------------------------------------------
// Panic handler.
//
// Only a binary may define `#[panic_handler]`, so the policy API lives in the
// library and the handler itself lives here.
// ---------------------------------------------------------------------------

use core::panic::PanicInfo;
use kernel::panic::{get_panic_policy, PanicPolicy};
use kernel::serial_println;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();

    serial_println!("\n========================================================");
    serial_println!("                 INTEROS CORE KERNEL PANIC                ");
    serial_println!("========================================================");
    if let Some(location) = info.location() {
        serial_println!("Origin:  {}:{}:{}", location.file(), location.line(), location.column());
    } else {
        serial_println!("Origin:  Unknown location");
    }
    serial_println!("Details: {}", info.message());

    // Diagnostic state capture. Locks are only attempted speculatively: a panic
    // must never deadlock behind a subsystem that was already faulting.
    let (heap_used, heap_free) = match kernel::allocator::try_heap_stats() {
        Some((used, free)) => (used, free),
        None => (0, 0),
    };
    let tasks = match kernel::task::SCHEDULER.try_lock() {
        Some(sched) => sched.task_count(),
        None => 0,
    };
    serial_println!("Diagnostic: heap_used={} heap_free={} tasks={}", heap_used, heap_free, tasks);

    match get_panic_policy() {
        PanicPolicy::Halt => {
            serial_println!("Action:  [PANIC POLICY: HALT] CPU halted safely to prevent state corruption.");
            serial_println!("========================================================\n");
            loop {
                x86_64::instructions::hlt();
            }
        }
        PanicPolicy::Reboot => {
            serial_println!("Action:  [PANIC POLICY: REBOOT] Resetting platform for automatic recovery...");
            serial_println!("========================================================\n");
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
            kernel::hal::traits::PowerController::reset(&kernel::hal::platform::POWER);
        }
    }
}
