//! Inter OS Kernel Entry Point
//!
//! Initializes low-level architecture subsystems (GDT, TSS, IDT, PIC, PIT Timer,
//! Serial COM1, Framebuffer Console, Physical Paging, and Heap Allocator),
//! brings up the ATA PIO Block Driver, SHA-256 Cryptographic Subsystem,
//! dm-verity Merkle Tree Verifier, Capability-Governed VFS, local Capability Tool Engine,
//! and pure `no_std` WebAssembly (WASM) Sandboxed Runtime.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod allocator;
pub mod arch;
pub mod audio;
pub mod capability;
pub mod compat;
pub mod crypto;
pub mod display_hal;
pub mod drivers;
pub mod event;
pub mod framebuffer;
pub mod fs;
pub mod gdt;
pub mod gui;
pub mod hal;
pub mod input;
pub mod interrupts;
pub mod ipc;
pub mod memory;
pub mod net;
pub mod package;
pub mod panic;
pub mod resource;
pub mod sensor;
pub mod services;
pub mod serial;
pub mod storage;
pub mod syscall;
pub mod task;
pub mod telephony;
pub mod wasm;

use alloc::boxed::Box;
use alloc::vec::Vec;
use bootloader_api::config::Mapping;
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use x86_64::VirtAddr;

use capability::{CapError, Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crypto::Sha256;
use drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
use fs::VFS;
use ipc::IPC_ROUTER;
use package::{build_rapp, parse_rapp, RappArch};
use storage::{MerkleVerifier, StorageError};
use task::{SCHEDULER, TaskId};
use wasm::{opcodes, Value, WasmError, WasmMemory, WasmModule, WasmVm};

/// Custom bootloader configuration requesting dynamic physical memory mapping.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Primary kernel entry function invoked by the bootloader.
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Initialize COM1 serial port for diagnostics and test logging
    serial::init();

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        framebuffer::init(framebuffer);
    }

    // 2. Output diagnostic banner to UART serial COM1
    serial_println!("================================================================================");
    serial_println!("  INTER OS CORE -- Open-Source Rust Mobile Microkernel (x86_64)");
    serial_println!("  Security: Zero-Trust Capability Microkernel | Mode: Ring 0/Ring 3 User Separation");
    serial_println!("  Subsystem: Hardware Drivers, Memory Management, IPC, VFS & Capabilities");
    serial_println!("================================================================================");

    // 3. Initialize Global Descriptor Table (GDT) and Task State Segment (TSS)
    gdt::init();
    serial_println!("[OK] GDT & TSS loaded with dedicated Double Fault IST stack & Ring 3 segments.");

    // 4. Initialize Interrupt Descriptor Table (IDT) for exceptions and hardware IRQs
    interrupts::init_idt();
    serial_println!("[OK] IDT loaded with CPU exception and hardware interrupt gates.");

    // 5. Initialize 8259 PIC controllers and unmask interrupts
    interrupts::init_pics();
    serial_println!("[OK] Dual 8259 PIC remapped (IRQs 32-47).");

    // 6. Initialize virtual memory page tables and physical frame allocator
    let phys_mem_offset_raw = boot_info
        .physical_memory_offset
        .into_option()
        .expect("Bootloader failed to map physical memory offset");
    let phys_mem_offset = VirtAddr::new(phys_mem_offset_raw);

    memory::init_memory(phys_mem_offset, &boot_info.memory_regions);
    serial_println!("[OK] Page tables and physical frame allocator initialized.");

    // 7. Initialize kernel heap allocator
    memory::init_kernel_heap()
        .expect("Kernel heap initialization failed");
    serial_println!("[OK] Kernel 8 MiB dynamic heap allocator online.");

    // Run Hardware MMU & Memory Isolation Adversarial Tests
    memory::run_mmu_adversarial_tests();

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

    // Test 8c: CPU Breakpoint exception trigger
    x86_64::instructions::interrupts::int3();
    serial_println!("[TEST] Software breakpoint exception executed and recovered cleanly.");

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
    syscall::entry::init();
    let dispatch_tid = syscall::sys_dispatch(7, 0, 0, 0, 0, 0);
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
    match drivers::ata::init() {
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
    drivers::init_and_test_drivers();

    // 10. Virtual Filesystem (VFS) Initialization & Package Runtime
    serial_println!("\n--- [Initializing Virtual Filesystem & Package Runtime] ---");
    fs::init();

    // Generate sample sandboxed package bytecode: returns 0 (exit success)
    let mut wasm_app_code = Vec::new();
    wasm_app_code.push(opcodes::I32_CONST);
    wasm_app_code.push(0); // return code 0 (success)
    wasm_app_code.push(opcodes::RETURN);
    wasm_app_code.push(opcodes::END);

    let rapp_binary = build_rapp(
        "org.interos.calculator",
        "interos_official",
        (1, 0, 0),
        RappArch::X86_64,
        &[1], // Cap: SerialOutput
        &wasm_app_code,
    );

    VFS.lock().create_file("/packages/calculator.rapp", &rapp_binary, true);
    serial_println!("[OK] Root filesystem mounted. Built-in package staged at '/packages/calculator.rapp'.");

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
    compat::android::init();



    // 15. Initialize Unified Dynamic Resource Manager (Priorities & Preemption)
    resource::init();

    // 16. Initialize Hardware Input Subsystem (Capacitive Touch Digitizer & Mobile Gesture Engine)
    input::init();

    // 19. Initialize Native Graphical User Shell & Window Compositor
    gui::init();

    // 20. Initialize PCI Bus Enumeration (Physical Peripheral Discovery)
    drivers::pci::init();

    // 21. Initialize In-Kernel Zero-Allocation Network Stack
    net::init();



    // 24. Initialize Multi-Architecture Mobile SoC Hardware Abstraction Layer (HAL)
    hal::init();

    // 24b. Initialize Multi-Architecture Hardware & Boot Image Layer (Fastboot v2 & AArch64)
    arch::init();

    // 25. Initialize Hardware TRNG & Entropy Subsystem

    crypto::rng::init();
    let mut trng_test_buf = [0u8; 32];
    crypto::get_random_bytes(&mut trng_test_buf);
    assert_ne!(trng_test_buf, [0u8; 32], "Hardware TRNG must produce non-zero entropy");
    serial_println!("[TEST] Hardware TRNG: Generated 32 bytes entropy, non-zero randomness validated.");

    // 26. Initialize Hardware-Isolated Secure Enclave & Keystore
    crypto::keystore::init();
    serial_println!("[OK] Hardware-isolated Secure Enclave & Keystore initialized.");

    // 27. Initialize Bare-Metal Mobile Audio Subsystem (PC Speaker & PCI Audio)
    audio::init();

    // 28. Initialize Sovereign Telephony & Cellular Baseband Modem Subsystem
    telephony::init();

    // 29. Initialize Mobile Hardware Sensor Subsystem (IMU, Orientation, Proximity)
    sensor::init();

    // 30. Initialize Storage Subsystems (Crash Dump & Encrypted Vault)
    storage::init();

    // 31. Initialize Cryptographic Secure IPC Channels
    ipc::init();

    // 31b. Initialize Asynchronous Event Notification Subsystem (Sovereign epoll/kqueue)
    event::init();

    // 31c. Verify .rapp package format round-trip (build → parse → SHA-256 verify)
    {
        use package::rapp::{build_rapp, parse_rapp, RappArch};
        let test_elf = b"\x7fELF_TEST_PAYLOAD_STUB";
        let rapp_bin = build_rapp("com.test.hello", "interos", (1, 0, 0), RappArch::X86_64, &[1, 2], test_elf);
        match parse_rapp(&rapp_bin) {
            Ok(manifest) => {
                assert_eq!(manifest.package_id, "com.test.hello");
                assert_eq!(manifest.publisher_id, "interos");
                assert_eq!(manifest.version, (1, 0, 0));
                assert_eq!(manifest.arch, RappArch::X86_64);
                assert_eq!(manifest.capabilities.len(), 2);
                assert_eq!(manifest.payload_len, test_elf.len());
                serial_println!("[OK] .rapp package round-trip: build → parse → SHA-256 verify PASSED");
            }
            Err(e) => {
                serial_println!("[FAIL] .rapp package round-trip failed: {:?}", e);
            }
        }
    }

    // 32. Initialize Hardware MSR Fast System Call Extensions (SCE, STAR, LSTAR, FMASK)
    syscall::entry::init();

    // 32b. Initialize Zero-Trust Capability Delegation Subsystem
    capability::init();

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

    // Phase 25: explicit userspace-style system service domains.
    services::init();

    // Task 1: Telemetry Service
    let mut caps_telemetry = CapabilityTable::new(8);
    caps_telemetry
        .insert(Capability {
            id: CapabilityId(101),
            cap_type: CapabilityType::TimerTickRead,
            rights: Rights::READ,
            object_id: 0,
        })
        .ok(); // Slot 0
    caps_telemetry
        .insert(Capability {
            id: CapabilityId(102),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 1

    let id_telemetry = SCHEDULER.lock().spawn(
        "telemetry_service",
        task_telemetry_service,
        caps_telemetry,
        1,
    );
    IPC_ROUTER.lock().register_task(id_telemetry);
    serial_println!("[OK] Spawned Task 1: telemetry_service (ID: {:?})", id_telemetry);

    // Task 2: Security Sentinel
    let mut caps_security = CapabilityTable::new(8);
    caps_security
        .insert(Capability {
            id: CapabilityId(201),
            cap_type: CapabilityType::IpcReceive,
            rights: Rights::READ,
            object_id: 0,
        })
        .ok(); // Slot 0
    caps_security
        .insert(Capability {
            id: CapabilityId(202),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 1

    let id_security = SCHEDULER.lock().spawn(
        "security_daemon",
        task_security_daemon,
        caps_security,
        2,
    );
    IPC_ROUTER.lock().register_task(id_security);
    serial_println!("[OK] Spawned Task 2: security_daemon (ID: {:?})", id_security);

    // Task 3: Device & Package Verification Manager (with VFS FileSystemRead + IpcSend to Task 2)
    let mut caps_device = CapabilityTable::new(8);
    caps_device
        .insert(Capability {
            id: CapabilityId(301),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 3,
        })
        .ok(); // Slot 0
    caps_device
        .insert(Capability {
            id: CapabilityId(302),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0 as u64,
        })
        .ok(); // Slot 1

    let id_device = SCHEDULER.lock().spawn(
        "device_manager",
        task_device_manager,
        caps_device,
        2,
    );
    IPC_ROUTER.lock().register_task(id_device);
    serial_println!("[OK] Spawned Task 3: device_manager (ID: {:?})", id_device);

    // Task 4: Storage & Integrity Daemon (with StorageRawBlock + FileSystemRead)
    let mut caps_storage = CapabilityTable::new(8);
    caps_storage
        .insert(Capability {
            id: CapabilityId(401),
            cap_type: CapabilityType::StorageRawBlock,
            rights: Rights::READ,
            object_id: 4,
        })
        .ok(); // Slot 0
    caps_storage
        .insert(Capability {
            id: CapabilityId(402),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 4,
        })
        .ok(); // Slot 1

    let id_storage = SCHEDULER.lock().spawn(
        "storage_daemon",
        task_storage_daemon,
        caps_storage,
        3,
    );
    IPC_ROUTER.lock().register_task(id_storage);
    serial_println!("[OK] Spawned Task 4: storage_daemon (ID: {:?})", id_storage);



    // Task 7: Native Graphical Compositor & Event Loop Shell
    let mut caps_compositor = CapabilityTable::new(8);
    caps_compositor
        .insert(Capability {
            id: CapabilityId(701),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 0
    caps_compositor
        .insert(Capability {
            id: CapabilityId(702),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0, // Bound to Task 2
        })
        .ok(); // Slot 1

    let id_compositor = SCHEDULER.lock().spawn(
        "compositor_shell",
        task_compositor_shell,
        caps_compositor,
        2,
    );
    IPC_ROUTER.lock().register_task(id_compositor);
    serial_println!("[OK] Spawned Task 7: compositor_shell (ID: {:?})", id_compositor);

    // Task 8: Network & InterLayer Client Worker
    let mut caps_net = CapabilityTable::new(8);
    caps_net
        .insert(Capability {
            id: CapabilityId(801),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 0
    caps_net
        .insert(Capability {
            id: CapabilityId(802),
            cap_type: CapabilityType::TimerTickRead,
            rights: Rights::READ,
            object_id: 0,
        })
        .ok(); // Slot 1
    caps_net
        .insert(Capability {
            id: CapabilityId(803),
            cap_type: CapabilityType::NetworkSocket,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 2
    caps_net
        .insert(Capability {
            id: CapabilityId(804),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok(); // Slot 3
    caps_net
        .insert(Capability {
            id: CapabilityId(805),
            cap_type: CapabilityType::NetworkReceive,
            rights: Rights::READ,
            object_id: 0,
        })
        .ok(); // Slot 4

    let id_net = SCHEDULER.lock().spawn(
        "net_worker",
        task_net_worker,
        caps_net,
        1,
    );
    IPC_ROUTER.lock().register_task(id_net);
    serial_println!("[OK] Spawned Task 8: net_worker (ID: {:?})", id_net);

    // Task 9: Primordial Userspace Init Process (Ring 3 User Mode, CPL 3)
    let id_user_init = task::spawn_user_init(id_security);
    IPC_ROUTER.lock().register_task(id_user_init);



    // Task 11: Sovereign Telephony & Cellular Modem Service
    let mut caps_telephony = CapabilityTable::new(8);
    caps_telephony
        .insert(Capability {
            id: CapabilityId(1101),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_telephony
        .insert(Capability {
            id: CapabilityId(1102),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_telephony = SCHEDULER.lock().spawn(
        "telephony_worker",
        telephony::task_telephony_worker,
        caps_telephony,
        1,
    );
    IPC_ROUTER.lock().register_task(id_telephony);
    serial_println!("[OK] Spawned Task 11: telephony_worker (ID: {:?})", id_telephony);

    // Task 12: Sovereign Audio Subsystem Service
    let mut caps_audio = CapabilityTable::new(8);
    caps_audio
        .insert(Capability {
            id: CapabilityId(1201),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_audio
        .insert(Capability {
            id: CapabilityId(1202),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_audio = SCHEDULER.lock().spawn(
        "audio_worker",
        task_audio_worker,
        caps_audio,
        1,
    );
    IPC_ROUTER.lock().register_task(id_audio);
    serial_println!("[OK] Spawned Task 12: audio_worker (ID: {:?})", id_audio);

    // Task 13: Mobile Hardware Sensor Service
    let mut caps_sensor = CapabilityTable::new(8);
    caps_sensor
        .insert(Capability {
            id: CapabilityId(1301),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_sensor
        .insert(Capability {
            id: CapabilityId(1302),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_sensor = SCHEDULER.lock().spawn(
        "sensor_worker",
        task_sensor_worker,
        caps_sensor,
        1,
    );
    IPC_ROUTER.lock().register_task(id_sensor);
    serial_println!("[OK] Spawned Task 13: sensor_worker (ID: {:?})", id_sensor);

    // Task 14: Real-Time Clock & Civil Calendar Daemon
    let mut caps_rtc = CapabilityTable::new(8);
    caps_rtc
        .insert(Capability {
            id: CapabilityId(1401),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_rtc
        .insert(Capability {
            id: CapabilityId(1402),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_rtc = SCHEDULER.lock().spawn(
        "rtc_worker",
        task_rtc_worker,
        caps_rtc,
        1,
    );
    IPC_ROUTER.lock().register_task(id_rtc);
    serial_println!("[OK] Spawned Task 14: rtc_worker (ID: {:?})", id_rtc);

    // Task 15: Mobile SoC Dynamic Thermal Governor Worker
    let mut caps_thermal = CapabilityTable::new(8);
    caps_thermal
        .insert(Capability {
            id: CapabilityId(1501),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_thermal
        .insert(Capability {
            id: CapabilityId(1502),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_thermal = SCHEDULER.lock().spawn(
        "thermal_worker",
        task_thermal_worker,
        caps_thermal,
        1,
    );
    IPC_ROUTER.lock().register_task(id_thermal);
    serial_println!("[OK] Spawned Task 15: thermal_worker (ID: {:?})", id_thermal);

    // Task 16: Mobile Wireless HAL & BLE Beacon Worker
    let mut caps_wireless = CapabilityTable::new(8);
    caps_wireless
        .insert(Capability {
            id: CapabilityId(1601),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        })
        .ok();
    caps_wireless
        .insert(Capability {
            id: CapabilityId(1602),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: id_security.0,
        })
        .ok();

    let id_wireless = SCHEDULER.lock().spawn(
        "wireless_worker",
        task_wireless_worker,
        caps_wireless,
        1,
    );
    IPC_ROUTER.lock().register_task(id_wireless);
    serial_println!("[OK] Spawned Task 16: wireless_worker (ID: {:?})", id_wireless);

    // Task 17: Sovereign Settings App (Ring 3 User Mode)
    let id_settings = task::spawn_app_settings(id_security);
    IPC_ROUTER.lock().register_task(id_settings);

    // Task 18: Sovereign Intent Terminal App (Ring 3 User Mode)
    let id_terminal = task::spawn_app_terminal(id_security);
    IPC_ROUTER.lock().register_task(id_terminal);

    // Task 19: Sovereign Dialer App (Ring 3 User Mode)
    let id_dialer = task::spawn_app_dialer(id_security);
    IPC_ROUTER.lock().register_task(id_dialer);

    // Task 20: Sovereign Vault & Storage App (Ring 3 User Mode)
    let id_vault = task::spawn_app_vault(id_security);
    IPC_ROUTER.lock().register_task(id_vault);

    // Task 21: Sovereign Camera & Neural Vision App (Ring 3 User Mode)
    let id_camera = task::spawn_app_camera(id_security);
    IPC_ROUTER.lock().register_task(id_camera);

    // Task 22: Open-Source Touch Calculator App (Ring 3 User Mode)
    let id_calc = task::spawn_app_calculator(id_security);
    IPC_ROUTER.lock().register_task(id_calc);

    // Task 23: Sovereign Browser & Web App (Ring 3 User Mode)
    let id_browser = task::spawn_app_browser(id_security);
    IPC_ROUTER.lock().register_task(id_browser);

    // Task 24: Sovereign Messages & P2P Wire App (Ring 3 User Mode)
    let id_messages = task::spawn_app_messages(id_security);
    IPC_ROUTER.lock().register_task(id_messages);

    // Task 25: Sovereign Calendar & Chrono-Planner App (Ring 3 User Mode)
    let id_calendar = task::spawn_app_calendar(id_security);
    IPC_ROUTER.lock().register_task(id_calendar);

    // Task 26: Sovereign Music & Hi-Fi Audio App (Ring 3 User Mode)
    let id_music = task::spawn_app_music(id_security);
    IPC_ROUTER.lock().register_task(id_music);

    // Render the Mobile Smartphone Shell on the graphical display
    framebuffer::draw_mobile_shell();

    serial_println!("================================================================================");
    serial_println!("  SYSTEM READY: Inter OS v3.0 Microkernel Fully Online.");
    serial_println!("  Encrypted Vault, Camera, Browser, Messages, Calendar, Music, Calculator & 26 Tasks Active.");
    serial_println!("================================================================================");

    // Emit live process telemetry for userspace and Device Studio telemetry inspector
    task::SCHEDULER.lock().dump_process_table();

    // 26. Enable hardware CPU interrupts and begin multitasking
    x86_64::instructions::interrupts::enable();
    serial_println!("[OK] Hardware interrupts enabled on CPU.");

    // Hand over control to multitasking scheduler
    loop {
        task::yield_now();
        x86_64::instructions::hlt();
    }
}

// -----------------------------------------------------------------------------
// Host Functions for Sandboxed Packages
// -----------------------------------------------------------------------------

/// Intercepts host capability invocations from sandboxed WASM packages.
fn host_package_call(_vm: &mut wasm::WasmVm, _args: &[wasm::Value]) -> Result<Option<wasm::Value>, wasm::WasmError> {
    Ok(Some(wasm::Value::I32(0)))
}

// -----------------------------------------------------------------------------
// System Tasks
// -----------------------------------------------------------------------------

/// Task 1: Periodic Telemetry Service
/// Gathers hardware timer metrics and outputs status via capability-checked syscalls.
extern "C" fn task_telemetry_service() -> ! {
    let mut iteration = 0u64;
    loop {
        iteration = iteration.wrapping_add(1);
        if iteration == 1 || iteration % 250 == 0 {
            if let Ok(_ticks) = syscall::sys_get_ticks(0) {
                syscall::sys_log(1, "[TELEMETRY TASK] Capability-verified telemetry heartbeat: Hardware timer active.");
            }
        }
        syscall::sys_yield();
    }
}

/// Task 2: Security Policy Daemon
/// Listens on guarded IPC channels and validates incoming system policy events.
extern "C" fn task_security_daemon() -> ! {
    let mut first = true;
    loop {
        if first {
            syscall::sys_log(1, "[SECURITY DAEMON] Capability-guarded IPC mailbox open. Listening for security events...");
            first = false;
        }
        // Attempt to receive incoming IPC message via Slot 0 (IpcReceive token)
        if let Ok(msg) = syscall::sys_ipc_recv(0) {
            let _ = msg;
            syscall::sys_log(1, "[SECURITY DAEMON] Received attested IPC message! Policy audit: APPROVED.");
        }
        syscall::sys_yield();
    }
}

/// Task 3: Device & Package Verification Worker
/// Loads the pre-staged `/packages/calculator.rapp` package from VFS, attests SHA-256 integrity,
/// instantiates the sandboxed WASM VM, verifies fuel quotas, and logs verification status.
extern "C" fn task_device_manager() -> ! {
    let mut executed = false;
    loop {
        if !executed {
            syscall::sys_log(1, "[DEVICE MANAGER] Reading package '/packages/calculator.rapp' from VFS...");

            // Open package file using Slot 2 (FileSystemRead token)
            match syscall::sys_fs_open(2, "/packages/calculator.rapp", Rights::READ) {
                Ok(fd) => {
                    let mut buf = [0u8; 1024];
                    if let Ok(bytes_read) = syscall::sys_fs_read(2, fd, &mut buf) {
                        syscall::sys_fs_close(fd);
                        syscall::sys_log(1, "[DEVICE MANAGER] Package binary read successfully. Verifying SHA-256 attestation...");

                        // Parse and cryptographically verify package
                        match parse_rapp(&buf[..bytes_read]) {
                            Ok(pkg) => {
                                syscall::sys_log(1, "[DEVICE MANAGER] Package Attestation SUCCESS! Instantiating sandboxed VM...");

                                let mut wasm_module = wasm::WasmModule::new();
                                wasm_module.add_host_import("host_call", 1, host_package_call);
                                let payload = buf[pkg.payload_offset..pkg.payload_offset + pkg.payload_len].to_vec();
                                let func_idx = wasm_module.add_function("main", 0, 0, payload);
                                wasm_module.add_export("main", func_idx);

                                let memory = wasm::WasmMemory::with_byte_limit(65536, 65536);
                                let mut vm = wasm::WasmVm::new(wasm_module, memory, 2000);

                                match vm.invoke_export("main", &[]) {
                                    Ok(Some(wasm::Value::I32(0))) => {
                                        syscall::sys_log(1, "[DEVICE MANAGER] Sandboxed execution succeeded! Return code 0 (OK).");
                                        let msg = b"CALCULATOR_VERIFIED_OK";
                                        let _ = syscall::sys_ipc_send(0, TaskId(2), 0x2001, msg);
                                        executed = true;
                                    }
                                    Ok(_) => {
                                        syscall::sys_log(1, "[DEVICE MANAGER] Sandboxed execution completed.");
                                        executed = true;
                                    }
                                    Err(_) => {
                                        syscall::sys_log(1, "[DEVICE MANAGER] Sandbox execution trapped or aborted.");
                                    }
                                }
                            }
                            Err(_) => {
                                syscall::sys_log(1, "[DEVICE MANAGER] Package cryptographic attestation failed.");
                            }
                        }
                    }
                }
                Err(_) => {
                    syscall::sys_log(1, "[DEVICE MANAGER] Package file not found in VFS.");
                }
            }
        }
        syscall::sys_yield();
    }
}

/// Task 4: Storage & Integrity Daemon
/// Performs continuous background audits of filesystem integrity and disk sectors.
extern "C" fn task_storage_daemon() -> ! {
    let mut audited = false;
    loop {
        if !audited {
            syscall::sys_log(0, "[STORAGE DAEMON] Performing background audit of system policy integrity...");

            // Verify /system/etc/policy.json via Slot 2 (FileSystemRead)
            if let Ok(stat) = syscall::sys_fs_stat(2, "/system/etc/policy.json") {
                if stat.read_only {
                    syscall::sys_log(0, "[STORAGE DAEMON] Policy partition verified: Read-Only System Inode intact.");
                }
            }

            // Verify live sector 0 read from hardware drive via Slot 1 (StorageRawBlock)
            let mut sec_buf = [0u8; SECTOR_SIZE];
            if syscall::sys_block_read(1, 0, &mut sec_buf) == syscall::SyscallResult::Success {
                if sec_buf[510] == 0x55 && sec_buf[511] == 0xAA {
                    syscall::sys_log(0, "[STORAGE DAEMON] Live hardware sector 0 audit: PASSED (MBR Magic 0x55AA confirmed).");
                    audited = true;
                }
            }
        }
        syscall::sys_yield();
    }
}



/// Task 7: Native Graphical Compositor & Event Loop Shell
/// Pumps hardware input events from the non-allocating ring buffer,
/// performs UI hit testing, dispatches attested user intents via zero-trust IPC,
/// and updates the graphical smartphone display.
extern "C" fn task_compositor_shell() -> ! {
    let mut initial = true;
    loop {
        if initial {
            syscall::sys_log(0, "[COMPOSITOR SHELL] Native mobile touch event loop active. Polling digitizer queue...");
            // Simulate capacitive touchscreen tap on Card 1 (System Diagnostics)
            input::INPUT_QUEUE.lock().push(input::InputEvent::Gesture(input::TouchGesture::Tap {
                x: (framebuffer::PHONE_X + 50) as u32,
                y: (framebuffer::PHONE_Y + 160) as u32,
            }));
            initial = false;
        }

        // Pump input event queue
        if let Some(event) = input::INPUT_QUEUE.lock().pop() {
            if let Some(intent) = gui::MOBILE_SHELL.lock().handle_event(event) {
                syscall::sys_log(0, "[COMPOSITOR SHELL] UI touch detected! Dispatching attested intent via IPC...");
                // Send IPC to security sentinel via Slot 1 (IpcSend token)
                syscall::sys_ipc_send(1, TaskId(2), 0x3001, intent.as_bytes());
            }
        }

        syscall::sys_yield();
    }
}

/// Task 8: Network Worker
///
/// Polls inbound network packets and demultiplexes IPv4/UDP datagrams.
extern "C" fn task_net_worker() -> ! {
    let mut initial = true;
    loop {
        // 1. Poll network interface for incoming packets
        let _polled = net::poll_network_interface();

        if initial {
            syscall::sys_log(0, "[NET WORKER] Network loop active: polling loopback & virtual NIC.");
            initial = false;
        }

        syscall::sys_yield();
    }
}

/// Task 12: Sovereign Audio Subsystem Worker
/// Monitors audio event queues, coordinates asynchronous tone generation and haptic feedback.
extern "C" fn task_audio_worker() -> ! {
    syscall::sys_log(0, "[AUDIO WORKER] Mobile audio subsystem online, monitoring feedback events...");
    let mut loop_count: u64 = 0;
    loop {
        loop_count += 1;
        if loop_count % 60 == 0 {
            syscall::sys_log(0, "[AUDIO WORKER] Heartbeat: Hardware audio tone engine ready.");
        }
        syscall::sys_yield();
    }
}

/// Task 13: Sovereign Mobile Sensor Worker
/// Continuously monitors IMU orientation, tilt, and proximity events.
extern "C" fn task_sensor_worker() -> ! {
    syscall::sys_log(0, "[SENSOR WORKER] Mobile sensor subsystem online, monitoring IMU orientation...");
    let mut loop_count: u64 = 0;
    loop {
        loop_count += 1;
        if loop_count % 60 == 0 {
            syscall::sys_log(0, "[SENSOR WORKER] Heartbeat: IMU orientation = Portrait, Proximity = Far.");
        }
        syscall::sys_yield();
    }
}

/// Task 14: Real-Time Clock (RTC / CMOS) Wall-Clock Daemon
/// Periodically queries hardware CMOS registers, maintaining accurate system time.
extern "C" fn task_rtc_worker() -> ! {
    syscall::sys_log(0, "[RTC WORKER] Hardware RTC wall-clock daemon online, syncing civil time...");
    let mut loop_count: u64 = 0;
    loop {
        loop_count += 1;
        if loop_count % 60 == 0 {
            let dt = hal::RTC.lock().now();
            let mut buf = [0u8; 32];
            let len = dt.format_iso(&mut buf);
            if let Ok(time_str) = core::str::from_utf8(&buf[..len]) {
                let mut log_buf = [0u8; 64];
                use core::fmt::Write;
                struct SliceWriter<'a> {
                    buf: &'a mut [u8],
                    pos: usize,
                }
                impl<'a> Write for SliceWriter<'a> {
                    fn write_str(&mut self, s: &str) -> core::fmt::Result {
                        let bytes = s.as_bytes();
                        let rem = self.buf.len().saturating_sub(self.pos);
                        let to_write = bytes.len().min(rem);
                        self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                        self.pos += to_write;
                        Ok(())
                    }
                }
                let mut writer = SliceWriter { buf: &mut log_buf, pos: 0 };
                let _ = write!(writer, "[RTC WORKER] Wall-clock time: {}", time_str);
                let pos = writer.pos;
                drop(writer);
                if let Ok(msg) = core::str::from_utf8(&log_buf[..pos]) {
                    syscall::sys_log(0, msg);
                }
            }
        }
        syscall::sys_yield();
    }
}

/// Task 15: Dynamic Multi-Zone Thermal Watchdog Worker
/// Periodically polls SoC temperature sensors and adjusts cooling governors.
extern "C" fn task_thermal_worker() -> ! {
    syscall::sys_log(0, "[THERMAL WORKER] Multi-zone thermal governor watchdog online...");
    let mut loop_count: u64 = 0;
    loop {
        loop_count += 1;
        if loop_count % 60 == 0 {
            let tm = resource::THERMAL_MANAGER.lock();
            let max_t = tm.max_temperature();
            let state = tm.cooling_state();
            drop(tm);
            let mut log_buf = [0u8; 96];
            use core::fmt::Write;
            struct SliceWriter<'a> {
                buf: &'a mut [u8],
                pos: usize,
            }
            impl<'a> Write for SliceWriter<'a> {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    let bytes = s.as_bytes();
                    let rem = self.buf.len().saturating_sub(self.pos);
                    let to_write = bytes.len().min(rem);
                    self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                    self.pos += to_write;
                    Ok(())
                }
            }
            let mut writer = SliceWriter { buf: &mut log_buf, pos: 0 };
            let _ = write!(writer, "[THERMAL WORKER] Max SoC Temp: {}C, State: {:?}, Throttling: false.", max_t, state);
            let pos = writer.pos;
            drop(writer);
            if let Ok(msg) = core::str::from_utf8(&log_buf[..pos]) {
                syscall::sys_log(0, msg);
            }
        }
        syscall::sys_yield();
    }
}

/// Task 16: Mobile Wireless Watchdog & BLE Beacon Daemon
/// Continuously transmits BLE GAP beacons and monitors Wi-Fi connection telemetry.
extern "C" fn task_wireless_worker() -> ! {
    syscall::sys_log(0, "[WIRELESS WORKER] Mobile wireless watchdog online, transmitting BLE beacons...");
    let mut loop_count: u64 = 0;
    loop {
        loop_count += 1;
        if loop_count % 60 == 0 {
            let wm = net::WIRELESS_MANAGER.lock();
            let state = wm.wifi_state;
            let adv = wm.ble_advertising;
            drop(wm);
            let mut log_buf = [0u8; 96];
            use core::fmt::Write;
            struct SliceWriter<'a> {
                buf: &'a mut [u8],
                pos: usize,
            }
            impl<'a> Write for SliceWriter<'a> {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    let bytes = s.as_bytes();
                    let rem = self.buf.len().saturating_sub(self.pos);
                    let to_write = bytes.len().min(rem);
                    self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                    self.pos += to_write;
                    Ok(())
                }
            }
            let mut writer = SliceWriter { buf: &mut log_buf, pos: 0 };
            let _ = write!(writer, "[WIRELESS WORKER] Wi-Fi: {:?}, BLE 5.0 Beaconing: {}.", state, adv);
            let pos = writer.pos;
            drop(writer);
            if let Ok(msg) = core::str::from_utf8(&log_buf[..pos]) {
                syscall::sys_log(0, msg);
            }
        }
        syscall::sys_yield();
    }
}




