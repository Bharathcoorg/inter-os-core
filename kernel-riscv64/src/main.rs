// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS — RISC-V 64 Sovereign Microkernel
//!
//! Targeting 64-bit RISC-V virt machine (QEMU virt / SiFive / StarFive / T-Head)
//! with active Sv39 MMU isolation, trap handling, and zero-trust system calls.

#![no_std]
#![no_main]

pub mod capability;
pub mod ipc;
pub mod entry;
pub mod sbi;
pub mod mmu;
pub mod trap;
pub mod syscall;
pub mod task;
pub mod elf;
pub mod images;
pub mod input_queue;
pub mod services;
pub mod allocator;
pub mod display;
pub mod pcie;

use core::panic::PanicInfo;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        let mut console = $crate::sbi::SbiConsole;
        let _ = core::fmt::write(&mut console, format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[no_mangle]
pub extern "C" fn kmain(hart_id: usize, dtb_addr: usize) -> ! {
    mmu::init_hart(hart_id);
    println!("");
    println!("============================================================");
    println!("  Inter OS — RISC-V 64 Sovereign Microkernel 0.1.0");
    println!("  Target: RISC-V 64 GC (QEMU Virt / Hardware Digital Twin)");
    println!("  Security: Pure Rust Microkernel with Capability Broker");
    println!("============================================================");
    println!("[BOOT] Booting on Hart ID: {}", hart_id);
    println!("[BOOT] Device Tree Blob (DTB) loaded at: 0x{:016x}", dtb_addr);

    // 1. Initialize Sv39 Virtual Memory Management
    unsafe {
        mmu::init_sv39();
    }
    println!("[BOOT] MMU: Sv39 3-level page tables active (SATP mode 8, W^X enforced).");

    // 2. Initialize S-mode Trap Vector Table & Stacks
    unsafe {
        trap::init_traps();
    }
    println!("[BOOT] Traps: stvec vector table online (direct mode, per-task sscratch).");

    // 2b. Initialize Dynamic Heap & Bochs Display Scanout
    allocator::init_heap();
    println!("[BOOT] RISC-V 32 MiB Dynamic Heap Allocator initialized.");
    pcie::probe_and_init_display();

    // Supervisor user-memory access must be off: firmware leaves SUM set, and
    // leaving it on would let S-mode reach user pages regardless of the
    // permissions encoded in the page tables.
    assert_eq!(
        trap::read_sstatus() & mmu::SSTATUS_SUM,
        0,
        "sstatus.SUM must be clear so user-page permissions are enforced"
    );
    println!("[TEST] Supervisor isolation: sstatus.SUM cleared after trap init (W^X enforced by hardware).");

    // 3. Test Security Boundary: Syscall pointer validation
    let mut tf = trap::TrapFrame::default();
    tf.regs[17] = 3; // Syscall 3 (Log)
    tf.regs[10] = 0; // cap_slot = 0
    tf.regs[11] = 0x80200000; // Pointer into KERNEL DRAM (forbidden!)
    tf.regs[12] = 32; // len = 32
    syscall::dispatch_syscall(&mut tf);
    assert_eq!(tf.regs[10] as isize, -14, "Kernel DRAM pointer MUST return -EFAULT!");
    println!("[TEST] RISC-V Security Invariant: Kernel address pointer rejected (-EFAULT).");

    // 4. Test Valid Syscall Execution
    let mut user_tf = trap::TrapFrame::default();
    user_tf.regs[17] = 7; // Syscall 7 (GetTaskId)
    syscall::dispatch_syscall(&mut user_tf);
    assert_eq!(user_tf.regs[10], 1, "GetTaskId should return task ID 1");
    println!("[TEST] RISC-V Userspace Syscall: SYS_GET_TASK_ID returned valid Task ID 1 (VERIFIED).");

    // 4b. Test Stub/Fail-Closed Capability Security for Standard Syscalls
    let mut cap_tf = trap::TrapFrame::default();
    for &sc in &[8, 9, 10, 11, 62, 71] {
        cap_tf.regs[17] = sc;
        syscall::dispatch_syscall(&mut cap_tf);
        assert_eq!(cap_tf.regs[10] as isize, -13, "Uncredentialed syscall must fail closed with -EACCES");
    }
    println!("[TEST] RISC-V Zero Ambient Authority: Syscalls 8, 9, 10, 11, 62, 71 fail closed (-EACCES) (VERIFIED).");

    // 4c. Test Task Scheduler Lifecycle & Removal Consistency
    task::test_task_lifecycle();

    // 4c-2. Test Floating Point & Callee-Saved Register Context Switch
    task::init_fp();
    task::test_context_switching();

    // 4d. Test Surface Authorization & Capability Separation
    {
        let mut sm = display::SURFACE_MANAGER.lock();
        let s_id = sm.create_surface(10, 100, 100, 1).expect("create surface");
        let pixels = [0xFF00FF00u32; 10];
        // Owner (10) can write
        assert!(sm.write_surface(s_id, 10, 0, &pixels).is_ok());
        // Unrelated task (11) CANNOT write
        assert_eq!(sm.write_surface(s_id, 11, 0, &pixels), Err(-13));
        // Unrelated task (11) without ADMIN CANNOT destroy
        assert_eq!(sm.destroy_surface(s_id, 11, false), Err(-13));
        // Unrelated task (11) WITH ADMIN CAN destroy
        assert_eq!(sm.destroy_surface(s_id, 11, true), Ok(()));
        println!("[TEST] RISC-V Surface Authorization: Unrelated task rejected (-EACCES), Admin override authorized VERIFIED.");
    }

    // 50-Cycle Memory Stability Test
    println!("--- [Testing RISC-V 50-Cycle Memory Stability] ---");
    let baseline_frames = mmu::active_allocated_frames();
    for cycle in 0..50 {
        let mut aspace = mmu::UserAddressSpace::new().expect("Address space allocation in stability cycle");
        let test_frame = mmu::allocate_frame().expect("Frame allocation in stability cycle");
        aspace.map_user_page(0x0040_0000, test_frame, true, false).expect("Mapping page in stability cycle");
        assert!(mmu::active_allocated_frames() > baseline_frames);
        assert!(aspace.destroy().is_ok());
        assert_eq!(mmu::active_allocated_frames(), baseline_frames, "Memory leak detected at cycle {}", cycle);
    }
    println!("[TEST] 50-Cycle Memory Stability: Free/used frame count returned to baseline ({}) across 50 cycles without leakage VERIFIED.", baseline_frames);

    // 50-Cycle Real Process Lifecycle Verification
    println!("--- [Testing RISC-V 50-Cycle Real Process Lifecycle] ---");
    static TEST_PROC_RISCV_ELF: [u8; 4096] = {
        let mut b = [0u8; 4096];
        b[0] = 0x7f; b[1] = 0x45; b[2] = 0x4c; b[3] = 0x46;
        b[4] = 0x02; b[5] = 0x01; b[6] = 0x01;
        b[16] = 0x02; b[18] = 0xf3; b[20] = 0x01;
        b[24] = 0x78; b[26] = 0x40; b[32] = 0x40;
        b[52] = 0x40; b[54] = 0x38; b[56] = 0x01; b[58] = 0x40;
        b[64] = 0x01; b[68] = 0x05; b[82] = 0x40; b[90] = 0x40;
        b[97] = 0x10; b[105] = 0x10; b[113] = 0x10;
        // Code at offset 120 (0x78)
        b[120] = 0x93; b[121] = 0x08; b[122] = 0x70; // li a7, 7 (SYS_GET_TASK_ID)
        b[124] = 0x73;                               // ecall
        b[128] = 0x93; b[129] = 0x08; b[130] = 0x20; // li a7, 2 (SYS_EXIT)
        b[132] = 0x13; b[133] = 0x05;               // li a0, 0 (exit code 0)
        b[136] = 0x73;                               // ecall
        b[140] = 0x6f;                               // j .
        b
    };

    let proc_baseline_frames = mmu::active_allocated_frames();
    let proc_baseline_tasks = task::SCHEDULER.lock().tasks.len();
    let kernel_satp = unsafe {
        let val: usize;
        core::arch::asm!("csrr {0}, satp", out(reg) val);
        val
    };
    unsafe { trap::IN_LIFECYCLE_TEST = true; }
    for cycle in 0..50 {
        let loaded = elf::load_elf(&TEST_PROC_RISCV_ELF).expect("Loading test ELF for lifecycle cycle");
        let parent_id = 99;
        let mut caps = capability::CapabilityTable::new();
        let _ = caps.mint(capability::CapabilityType::IpcSend, capability::Rights::INVOKE, None);
        let _ = caps.mint(capability::CapabilityType::IpcReceive, capability::Rights::READ, None);

        let (child_pid, child_satp) = {
            let mut sched = task::SCHEDULER.lock();
            let satp = loaded.address_space.satp;
            let pid = sched.spawn_user_process(parent_id, "test_child", loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
            assert!(pid > 0);
            (pid, satp)
        };

        // Activate child address space
        unsafe {
            mmu::switch_satp(child_satp);
        }
        {
            let mut sched = task::SCHEDULER.lock();
            if let Some(pos) = sched.tasks.iter().position(|t| t.id == child_pid) {
                sched.current_task_idx = pos;
                sched.tasks[pos].state = task::TaskState::Running;
            }
        }

        // Enter user mode (drops from S-mode to U-mode via sret)
        // In U-mode: executes SYS_GET_TASK_ID, then SYS_EXIT(0)
        // SYS_EXIT traps to S-mode, records exit code 0, marks child Terminated, and returns via return_to_kernel_mode()
        unsafe {
            trap::enter_user_mode_with_return(loaded.entry_point, loaded.stack_top, trap::kernel_stack_top());
        }

        // Switch back to kernel page table
        unsafe {
            mmu::switch_satp(kernel_satp);
        }

        assert_eq!(task::reap_task_locksafe(888, false, child_pid), Err(-13), "Non-child reap without supervisor rights must fail");
        let reaped_code = task::reap_task_locksafe(parent_id, false, child_pid).expect("Parent reap of terminated child must succeed");
        assert_eq!(reaped_code, 0);
        assert!(task::reap_task_locksafe(parent_id, false, child_pid).is_err(), "Double reap must fail");

        assert_eq!(mmu::active_allocated_frames(), proc_baseline_frames, "Process frame leak at cycle {}", cycle);
        assert_eq!(task::SCHEDULER.lock().tasks.len(), proc_baseline_tasks, "Process task leak at cycle {}", cycle);
    }
    unsafe { trap::IN_LIFECYCLE_TEST = false; }
    println!("[TEST] 50-Cycle Real Process Lifecycle: Spawn, execute, parent waitpid & zero frame/task leakage VERIFIED.");

    // Process Safety Invariants & Allocator Exhaustion/Rollback
    {
        // Quota exhaustion
        let mut sched = task::SCHEDULER.lock();
        let orig_len = sched.tasks.len();
        while sched.tasks.len() < task::MAX_TASKS {
            let pid = sched.allocate_pid().expect("allocate_pid");
            let _ = sched.add_task(task::TaskId(pid as usize), 0, 0x1000, 0x2000, 0x3000);
        }
        let excess_pid = sched.spawn_user_process(1, "overflow", 0x1000, 0x2000, None, capability::CapabilityTable::new());
        assert_eq!(excess_pid, 0, "Spawn beyond MAX_TASKS must return 0");
        while sched.tasks.len() > orig_len {
            sched.tasks.pop();
        }
        drop(sched);

        // Active process reap denial (-EBUSY)
        let mut sched = task::SCHEDULER.lock();
        let active_pid = sched.spawn_user_process(99, "active_proc", 0x1000, 0x2000, None, capability::CapabilityTable::new());
        if let Some(t) = sched.tasks.iter_mut().find(|t| t.id == active_pid) {
            t.state = task::TaskState::Running;
        }
        let reap_active_res = sched.reap_task(99, false, active_pid);
        assert!(reap_active_res == Err(-16) || reap_active_res == Err(-30), "Reaping non-terminated/running task must be rejected (-EBUSY or -30)");
        if let Some(t) = sched.tasks.iter_mut().find(|t| t.id == active_pid) {
            t.state = task::TaskState::Terminated;
        }
        assert!(sched.reap_task(99, false, active_pid).is_ok());
        drop(sched);

        // Partial ELF load rollback
        let corrupt_elf = [0x7fu8, b'E', b'L', b'F', 0, 0, 0];
        let baseline = mmu::active_allocated_frames();
        assert!(elf::load_elf(&corrupt_elf).is_err());
        assert_eq!(mmu::active_allocated_frames(), baseline, "ELF load failure must roll back all frames");
    }
    println!("[TEST] Process Safety Invariants: Quota exhaustion, active reap rejection & ELF rollback VERIFIED.");

    // Physical Frame Ownership & Reference Counting
    {
        println!("--- [Testing RISC-V Physical Frame Ownership & Shared Mappings] ---");
        let baseline = mmu::active_allocated_frames();
        let frame1 = mmu::allocate_frame().expect("Frame alloc");
        let mut aspace_a = mmu::UserAddressSpace::new().expect("Aspace A");
        let mut aspace_b = mmu::UserAddressSpace::new().expect("Aspace B");

        // Atomic lifecycle & race tests:
        assert_eq!(aspace_a.is_closing(), false);
        assert_eq!(aspace_a.try_activate(), Ok(()));
        assert!(aspace_a.is_active_on_any_hart());
        aspace_a.deactivate();
        assert_eq!(aspace_a.is_active_on_any_hart(), false);

        // Private mapping
        assert_eq!(aspace_a.map_private_frame(0x0040_0000, frame1, true, false), Ok(()));
        assert_eq!(mmu::frame_ownership(frame1), Some(mmu::FrameOwnership::PrivateOwned));
        // Private frame already mapped must be rejected if remapped
        assert!(aspace_b.map_private_frame(0x0040_0000, frame1, true, false).is_err(), "Private frame already mapped must be rejected");

        // Shared mapping: frame2 mapped into A (refcount 1), then into B (refcount 2)
        let frame2 = mmu::allocate_frame_with_ownership(mmu::FrameOwnership::Shared).expect("Frame alloc");
        assert_eq!(aspace_a.map_shared_frame(0x0050_0000, frame2, false, false), Ok(()));
        assert_eq!(aspace_b.map_shared_frame(0x0050_0000, frame2, false, false), Ok(()));
        assert_eq!(mmu::frame_ownership(frame2), Some(mmu::FrameOwnership::Shared));
        assert_eq!(mmu::frame_refcount(frame2), 2);

        // Borrowed mapping: registered physical buffer in PCIe aperture
        assert_eq!(aspace_a.map_borrowed_frame(0x0060_0000, 0x6000_0000, true, false), Ok(()));

        // Rejection of invalid unregistered MMIO address
        assert!(aspace_a.map_borrowed_frame(0x0070_0000, 0x0005_0000, true, false).is_err(), "Unregistered MMIO address must be rejected");

        // Destroy Aspace A: private frame1 freed, shared frame2 retained (refcount 1), borrowed not freed
        assert!(aspace_a.destroy().is_ok());
        assert_eq!(mmu::frame_ownership(frame2), Some(mmu::FrameOwnership::Shared));
        assert_eq!(mmu::frame_refcount(frame2), 1);
        // Activation after close/destroy must be rejected
        assert_eq!(aspace_a.try_activate(), Err(mmu::ActivationError::ClosingOrDestroyed));

        // Destroy Aspace B: shared frame2 drops to 0 and is freed
        assert!(aspace_b.destroy().is_ok());

        assert_eq!(mmu::active_allocated_frames(), baseline, "All owned frames must return to baseline");

        // Quarantine metadata & diagnostic tracking verification
        let q_before = mmu::quarantined_frame_count();
        mmu::quarantine_frame_with_meta(0x1234_0000, mmu::FrameOwnership::PrivateOwned, "test quarantine metadata");
        assert_eq!(mmu::quarantined_frame_count(), q_before + 1);
        {
            let q_lock = mmu::QUARANTINED_FRAMES.lock();
            let entry = q_lock.last().unwrap();
            assert_eq!(entry.paddr, 0x1234_0000);
            assert_eq!(entry.ownership, mmu::FrameOwnership::PrivateOwned);
            assert_eq!(entry.reason, "test quarantine metadata");
        }

        println!("[TEST] Frame Ownership & Shared Mappings: Private/Shared/Borrowed lifecycle & Quarantine Metadata VERIFIED.");
    }

    // MMU Ownership Reclamation Errors & Quarantine Verification
    mmu::test_mmu_ownership_reclamation_errors();

    // Service Supervisor Lifecycle & Recovery Verification
    services::test_service_supervisor_lifecycle_hardening();
    let test_svc_id = 3; // storaged
    let orig_pid = services::start(test_svc_id).expect("Service start failed in heartbeat test");
    assert!(orig_pid > 0);
    let cur_gen = services::service_generation(test_svc_id).expect("valid service generation");
    assert_eq!(services::start(test_svc_id), Err(()), "Duplicate start of running service must be rejected");
    assert_eq!(services::heartbeat(9999, cur_gen), Err(()), "Heartbeat with spoofed PID must be rejected");
    assert_eq!(services::heartbeat(orig_pid, cur_gen), Ok(()), "Heartbeat with owning PID and valid generation must be accepted");
    assert_eq!(services::heartbeat(orig_pid, 0), Err(()), "Heartbeat with unvalidated/zero generation must be rejected");
    assert_eq!(services::heartbeat(orig_pid, cur_gen.wrapping_add(999)), Err(()), "Heartbeat with stale generation must be rejected");
    assert_eq!(services::stop(test_svc_id), Ok(()), "Service clean-up stop must succeed");
    {
        let mut sched = task::SCHEDULER.lock();
        sched.tasks.clear();
        sched.next_pid = 1;
    }
    println!("[TEST] Service Supervisor Lifecycle: Stopped -> Running -> Heartbeat -> Crashed -> Recover(Restarting -> Running) -> Stopped VERIFIED.");

    println!("[BOOT] InterOS RISC-V SBI console/trap interface initialized.");
    println!("[BOOT] Microkernel S-mode initialized.");
    println!("[BOOT] RISC-V userspace smoke image is the current architecture validation target.");
    println!("[BOOT] System state: U-mode isolation smoke path ready; runtime validation remains required.");
    println!("------------------------------------------------------------");

    // 5. Deploy and Launch Real RISC-V U-Mode Userspace Process
    println!("--- [Launching RISC-V U-Mode Primordial Init Process] ---");

    // Assemble native RISC-V U-mode machine code payload:
    // 1. SYS_GET_TASK_ID (li a7, 7; ecall)
    // 2. SYS_LOG (li a7, 3; li a0, 0; lui a1, 0x40000; addi a1, a1, 0x30; li a2, 38; ecall)
    // 3. SYS_EXIT (li a7, 2; li a0, 0; ecall)
    // 4. loop
    let mut payload = [0u8; 128];
    let msg = b"Hello from InterOS RISC-V Userspace!\n";
    let code_words: [u32; 12] = [
        0x00700893, // li a7, 7 (SYS_GET_TASK_ID)
        0x00000073, // ecall
        0x00300893, // li a7, 3 (SYS_LOG)
        0x00000513, // li a0, 0 (cap_slot 0)
        0x400005b7, // lui a1, 0x40000 (base 0x4000_0000)
        0x03058593, // addi a1, a1, 0x30 (ptr 0x4000_0030)
        (msg.len() as u32) << 20 | (0 << 15) | (0 << 12) | (12 << 7) | 0x13, // li a2, msg_len
        0x00000073, // ecall
        0x00200893, // li a7, 2 (SYS_EXIT)
        0x00000513, // li a0, 0 (exit_code 0)
        0x00000073, // ecall
        0x0000006f, // j .
    ];

    for (i, word) in code_words.iter().enumerate() {
        let bytes = word.to_le_bytes();
        payload[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
    }

    payload[48..48 + msg.len()].copy_from_slice(msg);

    unsafe {
        mmu::load_user_code(&payload);
    }

    let satp = unsafe {
        let value: usize;
        core::arch::asm!("csrr {0}, satp", out(reg) value);
        value
    };
    let registered = task::SCHEDULER.lock().add_task(
        task::TaskId(1),
        satp,
        mmu::USER_CODE_VADDR,
        mmu::USER_STACK_TOP,
        trap::kernel_stack_top(),
    );
    assert!(registered, "RISC-V primordial userspace task must register with scheduler");

    println!("[INIT] Loaded native U-mode payload at VA 0x{:08x} (W^X RX code segment).", mmu::USER_CODE_VADDR);
    println!("[INIT] Initialized U-mode stack at VA 0x{:08x} (W^X RW stack segment).", mmu::USER_STACK_TOP);
    println!("[INIT] Dropping CPU execution privilege from S-mode to U-mode via sret...");

    unsafe {
        trap::enter_user_mode(mmu::USER_CODE_VADDR, mmu::USER_STACK_TOP, trap::kernel_stack_top());
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n[KERNEL PANIC] RISC-V 64 Kernel halted: {}", info);
    sbi::shutdown();
}
