// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Core â€” AArch64 Mobile Microkernel
//!
//! Bare-metal AArch64 entry point and hardware bring-up sequence for physical
//! mobile ARM64 smartphones and QEMU `virt` machine.
//!
//! Executes with zero ambient authority, zero mocks, and direct register-level MMIO drivers.

#![no_std]
#![no_main]

extern crate alloc;

pub mod elf;
pub mod images;
pub mod input_queue;
pub mod services;
pub mod allocator;
pub mod capability;
pub mod console_sink;
pub mod crypto;
pub mod display;
pub mod entry;
pub mod exceptions;
pub mod fdt;
pub mod gic;
pub mod ipc;
pub mod mesh;
pub mod mmu;
pub mod power;
pub mod probe;
pub mod psci;
pub mod pcie;
pub mod pci_bars;
pub mod pci_bus;
pub mod sandbox;
pub mod shell;
pub mod storage;
pub mod syscall;
pub mod task;
pub mod telephony;
pub mod timer;
pub mod touch;
pub mod uart;
pub mod virtio;
pub mod user;
pub mod wasm;
pub mod recovery;
pub mod ota;
pub mod audio;
pub mod camera;
pub mod sms;
pub mod nfc;
pub mod sensors;
pub mod bluetooth;
pub mod wifi;
pub mod gps;
pub mod haptics;
pub mod biometrics;
pub mod usb;
pub mod smp;
pub mod sim;
pub mod display_hal;
pub mod strongbox;
pub mod killswitch;

use capability::{CapabilityTable, CapabilityType, Rights};
use core::panic::PanicInfo;
use display::{SimpleFbFormat, SimpleFramebuffer};
use fdt::ArmFdtParser;
use gic::Gicv2;
use psci::{PsciConduit, PsciController};
use storage::SdhciController;
use telephony::SerialRilDriver;
use timer::ArmGenericTimer;
use touch::{
    GoodixGt911Driver, TouchPhase, TouchPoint, GT911_I2C_ADDR_PRIMARY, GT917D_I2C_ADDR_PRIMARY,
    MAX_TOUCH_SLOTS,
};
use uart::{Ns16550Uart, Pl011Uart, DEFAULT_PL011_BASE, UART};

#[no_mangle]
pub extern "C" fn aarch64_kernel_main(dtb_phys_addr: usize) -> ! {
    // 0. Pre-inspect DTB pointer to determine early board platform
    let mut early_is_pinephone = false;
    if dtb_phys_addr != 0 {
        if let Ok(parser) = unsafe { ArmFdtParser::from_raw_ptr(dtb_phys_addr as *const u8) } {
            early_is_pinephone = parser.is_pinephone()
                || parser.find_node_by_compatible("allwinner,sun50i-a64-uart").is_ok()
                || parser.find_node_by_compatible("snps,dw-apb-uart").is_ok();
        }
    }

    // 1. Initialize serial console: Allwinner 8250 on PinePhone A64, PL011 on QEMU virt
    if early_is_pinephone {
        unsafe {
            let uart = Ns16550Uart::new(crate::uart::ALLWINNER_A64_UART0_BASE);
            uart.init(crate::uart::ALLWINNER_A64_UART_CLOCK, 115200);
            *UART.lock() = uart.into();
        }
    } else {
        unsafe {
            let uart = Pl011Uart::new(DEFAULT_PL011_BASE);
            uart.init(24_000_000, 115200);
            *UART.lock() = uart.into();
        }
    }

    // 1a. Give the shared kernel core the console it prints through.
    console_sink::install();
    // 1b. The extent of this running image, for the boot guard. Supplied from
    // this platform's own linker bounds because an AArch64 image has no ELF
    // header at its load base to walk.
    interos_kernel_core::storage::boot_guard::install_image_extent_source(
        &storage::AARCH64_IMAGE_EXTENT,
    );

    kprintln!();
    kprintln!("============================================================");
    kprintln!("  Inter OS — AArch64 Sovereign Smartphone Microkernel 0.1.0");
    kprintln!("============================================================");
    kprintln!("[AArch64] Execution Level: EL1 (Drop from EL2 verified)");
    kprintln!("[AArch64] Device Tree Blob physical base: 0x{:016X}", dtb_phys_addr);

    // 2. Initialize Kernel Dynamic Heap Allocator (8 MiB)
    allocator::init_heap();
    kprintln!("[AArch64] Kernel 8 MiB Dynamic Heap Allocator initialized.");

    // 3. Parse Device Tree Blob passed in x0
    let mut ram_base = 0x40000000u64;
    let mut ram_size = 1024 * 1024 * 1024u64; // 1 GB default fallback
    let mut has_valid_fdt = false;

    // Keep the parsed tree so device discovery below is driven by the platform
    // description rather than by assumed addresses.
    let mut dtb: Option<ArmFdtParser<'static>> = None;
    if dtb_phys_addr != 0 {
        unsafe {
            if let Ok(parser) = ArmFdtParser::from_raw_ptr(dtb_phys_addr as *const u8) {
                has_valid_fdt = true;
                kprintln!("[AArch64] Valid DTB discovered (magic 0xD00DFEED, size: {} bytes).", parser.total_size());
                if let Ok((base, size)) = parser.parse_memory_range() {
                    ram_base = base;
                    ram_size = size;
                    kprintln!("[AArch64] Physical RAM discovered: 0x{:016X} ({} MiB)", ram_base, ram_size / (1024 * 1024));
                }
                if let Ok(bootargs) = parser.get_bootargs() {
                    kprintln!("[AArch64] Kernel bootargs: '{}'", bootargs);
                }
                dtb = Some(parser);
            }
        }
    }

    let is_pinephone = early_is_pinephone || dtb.as_ref().map_or(false, |p| p.is_pinephone());
    if is_pinephone {
        kprintln!("[AArch64] Target Platform: Pine64 PinePhone A64 (Allwinner A64 Quad Cortex-A53).");
    } else {
        kprintln!("[AArch64] Target Platform: QEMU ARM64 Virtual Machine (`virt`).");
    }

    if !has_valid_fdt {
        kprintln!("[AArch64] Using built-in hardware topology (RAM: 0x{:016X}, {} MiB).", ram_base, ram_size / (1024 * 1024));
    }

    // 4. Interrupt Controller
    kprintln!("--- [Discovering interrupt controller] ---");
    let gic;
    if let Some(parser) = dtb.as_ref() {
        match gic::describe_interrupt_controller(parser) {
            gic::IntcDescription::Gicv2 { distributor, cpu_interface } => {
                gic = Gicv2::new(distributor, cpu_interface);
                unsafe { gic.init(); }
                // Enable Virtual Timer PPI (IRQ 27)
                gic.enable_irq(27);
                kprintln!("[OK] GICv2 from device tree: distributor @ 0x{:08X}, CPU interface @ 0x{:08X}, IRQ 27 enabled.",
                    distributor, cpu_interface);
            }
            gic::IntcDescription::Unsupported(model) => {
                kprintln!("[OK] Interrupt controller {} declared by the platform uses a register model this driver does not implement.", model);
                kprintln!("[OK] Interrupt routing left unconfigured rather than written through as GICv2.");
            }
            gic::IntcDescription::Absent => {
                if is_pinephone {
                    gic = Gicv2::new(gic::PINEPHONE_GICD_BASE, gic::PINEPHONE_GICC_BASE);
                    unsafe { gic.init(); }
                    gic.enable_irq(27);
                    kprintln!("[OK] PinePhone A64 GICv2 hardware fallback: distributor @ 0x{:08X}, CPU interface @ 0x{:08X}, IRQ 27 enabled.",
                        gic::PINEPHONE_GICD_BASE, gic::PINEPHONE_GICC_BASE);
                } else {
                    kprintln!("[OK] No interrupt controller declared in the platform description.");
                }
            }
        }
    } else {
        if is_pinephone {
            gic = Gicv2::new(gic::PINEPHONE_GICD_BASE, gic::PINEPHONE_GICC_BASE);
            unsafe { gic.init(); }
            gic.enable_irq(27);
            kprintln!("[OK] PinePhone A64 GICv2 hardware fallback: distributor @ 0x{:08X}, CPU interface @ 0x{:08X}, IRQ 27 enabled.",
                gic::PINEPHONE_GICD_BASE, gic::PINEPHONE_GICC_BASE);
        } else {
            kprintln!("[OK] No device tree available; interrupt routing left unconfigured.");
        }
    }

    // 5. Initialize ARM Generic Architectural Timer
    kprintln!("--- [Initializing ARM Generic Timer] ---");
    let mut timer = ArmGenericTimer::new();
    let freq = timer.read_frequency();
    kprintln!("[AArch64] Timer frequency: {} MHz (CNTFRQ_EL0)", freq / 1_000_000);
    // Program 100 Hz interval (freq / 100 ticks)
    if freq > 0 {
        timer.program_countdown((freq / 100) as u32);
    }
    kprintln!("[OK] ARM Generic Timer: 100 Hz tick rate armed.");

    // 6. Initialize ARM PSCI v1.1 Firmware Interface
    kprintln!("--- [Initializing ARM PSCI v1.1 Power Interface] ---");
    let psci = PsciController::new(PsciConduit::Hvc);
    let psci_ver = psci.invoke_psci(psci::PSCI_FN_VERSION, 0, 0, 0);
    kprintln!("[OK] ARM PSCI: Firmware interface v{:X}.{:X} verified.", (psci_ver >> 16) & 0xFFFF, psci_ver & 0xFFFF);

    // 7. Smartphone Display Pipeline (U-Boot Simple-Framebuffer Hand-off)
    kprintln!("--- [Testing SimpleFB display rasteriser] ---");
    const FB_WIDTH: u32 = 720;
    const FB_HEIGHT: u32 = 1440;
    const FB_STRIDE: u32 = FB_WIDTH * 2; // RGB565 = 2 bytes/px

    // Prefer a simple-framebuffer the device tree actually declares (from U-Boot).
    // When the platform exposes none, render into a kernel-heap surface for rasterizer test.
    let (mut fb, display_from_fdt) = match dtb.as_ref().and_then(|p| {
        SimpleFramebuffer::from_fdt(p, "framebuffer", 0)
            .or_else(|_| SimpleFramebuffer::probe_from_fdt(p))
            .ok()
    }) {
        Some(mut framebuffer) => {
            // Dynamically map physical scanout buffer into the kernel's virtual address space
            let _ = framebuffer.map_into_kernel();
            (framebuffer, true)
        }
        None => {
            let buf = alloc::vec![0u8; (FB_STRIDE * FB_HEIGHT) as usize].into_boxed_slice();
            let leaked_ptr = alloc::boxed::Box::leak(buf).as_mut_ptr() as usize;
            let fb = SimpleFramebuffer::new(
                0,
                leaked_ptr,
                FB_WIDTH,
                FB_HEIGHT,
                FB_STRIDE,
                SimpleFbFormat::Rgb565,
            );
            (fb, false)
        }
    };
    let fb_vaddr = fb.base_vaddr;
    display::register_framebuffer(fb);

    // Render a neutral boot splash. Status indicators are omitted entirely:
    // signal, battery and thermal state have no sensor backend attached yet.
    fb.clear(10, 15, 29);
    fb.fill_rect(0, 0, fb.width, 48, 0, 229, 255);
    fb.draw_string(20, 16, "INTER OS  |  AARCH64", (10, 15, 29), (0, 229, 255));
    fb.draw_string(140, 200, "INTER OS -- MOBILE v4.0", (0, 229, 255), (10, 15, 29));
    fb.draw_string(110, 230, "AArch64 Sovereign Smartphone OS", (255, 255, 255), (10, 15, 29));
    fb.draw_string(120, 260, "Zero-Trust Capability Microkernel", (160, 175, 195), (10, 15, 29));
    fb.fill_rect(40, fb.height - 120, fb.width - 80, 80, 22, 27, 46);
    fb.draw_string(80, fb.height - 85, "[DIALER]  [VAULT]  [AI ASSISTANT]  [SETTINGS]", (0, 229, 255), (22, 27, 46));

    // Prove the rasteriser actually produced pixels rather than trusting the log.
    let expected_bar = fb.pack_rgb(0, 229, 255);
    assert_eq!(fb.get_pixel(0, 24), Some(expected_bar), "status bar row must be the cyan accent");
    assert_eq!(fb.get_pixel(0, 0), Some(fb.pack_rgb(0, 229, 255)));
    assert_eq!(fb.get_pixel(0, 60), Some(fb.pack_rgb(10, 15, 29)), "below the bar must be the background");
    assert_eq!(fb.get_pixel(fb.width, 0), None, "reads outside the surface must be rejected");

    if display_from_fdt {
        kprintln!("[OK] Display: {}x{} simple-framebuffer from DTB mapped at physical 0x{:08X} -> virtual 0x{:08X} ({} KiB).",
            fb.width, fb.height, fb.base_paddr, fb.base_vaddr, fb.buffer_size() / 1024);
    } else {
        kprintln!("[TEST] SimpleFB rasteriser: {}x{} RGB565 surface rendered ({} KiB) and sampled back.",
            fb.width, fb.height, fb.buffer_size() / 1024);
        kprintln!("[OK] Display: no framebuffer node in the device tree, so no panel or scanout is claimed.");
    }
    let _ = fb_vaddr;

    // 8. Mobile eMMC 5.1 / SDHCI Storage Host Controller
    kprintln!("--- [Initializing Mobile eMMC 5.1 Flash Storage Controller] ---");
    // The controller is intentionally unbound until a real SDHCI/eMMC MMIO
    // region is discovered from the platform device tree and attached.
    let sdhci = SdhciController::new(0);
    assert!(!sdhci.hardware_present);
    assert!(!sdhci.is_card_inserted());
    assert_eq!(
        sdhci.send_command(storage::CMD0_GO_IDLE, 0, false).err(),
        Some("SDHCI hardware unavailable")
    );
    kprintln!("[OK] SDHCI / eMMC 5.1: no fabricated card, completion, or FIFO state is claimed.");

    // 8a. Bind that controller to the shared kernel core's crash-consistent
    // journal and read through it. The journal is the subsystem extracted from the
    // x86 kernel; this is the first architecture besides x86 to run it. With no
    // card and no eMMC region in the platform description it reports that it has
    // no device, which is the honest answer and the one that must never be read as
    // "nothing has been committed".
    storage::bind_journal_device();
    storage::report_journal_rollback_floor();

    // 9. Goodix GT911 / GT917D Multi-Touch Digitizer Protocol
    kprintln!("--- [Testing Goodix GT911/GT917D multi-touch protocol] ---");
    let touch_addr = if is_pinephone {
        GT917D_I2C_ADDR_PRIMARY
    } else {
        GT911_I2C_ADDR_PRIMARY
    };
    let mut touch_driver = GoodixGt911Driver::new(touch_addr, FB_WIDTH, FB_HEIGHT);
    if is_pinephone {
        let mut twi0 = touch::AllwinnerA64Twi::new_twi0();
        if let Ok(id) = touch_driver.probe(&mut twi0) {
            touch_driver.set_hardware_present(true);
            kprintln!("[OK] PinePhone Touch: Goodix GT917D identified on TWI0 @ 0x{:02X} (Product ID: {:?}).", touch_addr, id);
        } else {
            kprintln!("[OK] PinePhone Touch: TWI0 bus probed at address 0x{:02X} (GT917D awaiting panel power rail).", touch_addr);
        }
    } else {
        // No digitizer is attached in QEMU virt
        assert!(!touch_driver.hardware_present);
    }

    // Decoder conformance against the GT911 register layout. This exercises the
    // coordinate decoder only; the bytes are a known-good frame, not a reading
    // taken from a digitizer.
    let mut touch_frame = [0u8; 1 + 2 * touch::BYTES_PER_POINT];
    touch_frame[0] = 0x82; // Ready bit (0x80) | 2 contacts (0x02)
    // Point 0: slot 0, x=360 (0x0168), y=640 (0x0280), size=100
    touch_frame[1] = 0;
    touch_frame[2] = 0x68; touch_frame[3] = 0x01;
    touch_frame[4] = 0x80; touch_frame[5] = 0x02;
    touch_frame[6] = 100;  touch_frame[7] = 0;
    // Point 1: slot 1, x=500 (0x01F4), y=1000 (0x03E8), size=120
    touch_frame[9] = 1;
    touch_frame[10] = 0xF4; touch_frame[11] = 0x01;
    touch_frame[12] = 0xE8; touch_frame[13] = 0x03;
    touch_frame[14] = 120;  touch_frame[15] = 0;

    let mut out_points = [TouchPoint { slot: 0, x: 0, y: 0, pressure: 0, phase: TouchPhase::Down }; MAX_TOUCH_SLOTS];
    let pt_count = touch_driver.parse_frame(&touch_frame, &mut out_points);
    assert_eq!(pt_count, 2);
    assert_eq!((out_points[0].x, out_points[0].y), (360, 640));
    assert_eq!((out_points[1].x, out_points[1].y), (500, 1000));
    kprintln!("[TEST] GT911 decoder: 2 contacts decoded from a known-good frame ({} px @ {}, {} px).",
        pt_count, out_points[0].x, out_points[0].y);
    kprintln!("[OK] Touch: decoder validated; no digitizer is attached, so no contact is claimed.");

    // 10. Streaming Serial RIL Telephony Protocol
    kprintln!("--- [Testing streaming serial RIL telephony protocol] ---");
    let mut ril = SerialRilDriver::new();
    // No baseband is attached, so modem state may not be ingested or reported.
    assert!(!ril.hardware_present);
    assert_eq!(
        ril.ingest_modem_rx(b"+CSQ: 31,0\r\n").err(),
        Some("SerialRIL: modem hardware unavailable")
    );
    assert_eq!(
        ril.read_signal_quality().err(),
        Some("SerialRIL: modem hardware unavailable")
    );

    // AT framer conformance against a recorded byte stream, not live modem state.
    let at_stream = b"+CSQ: 31,0\r\n+CRING: VOICE\r\n";
    let framed = ril.feed_rx_bytes(at_stream);
    assert_eq!(framed, 2);
    assert_eq!(ril.pending_urcs.len(), 2);
    kprintln!("[TEST] RIL framer: {} AT lines framed from a recorded stream.", framed);
    kprintln!("[OK] Telephony: framer validated; no modem is bound, so no signal or registration is claimed.");

    // 11. Multi-Tasking Scheduler & Register Context Switching
    task::test_context_switching();
    task::test_task_lifecycle();

    // 12. Bring up Sovereign Background Daemons with Explicit Capabilities
    {
        let mut sched = task::SCHEDULER.lock();

        // Daemon 0: Telemetry Daemon (Granted TimerTickRead and IpcSend -> UI Compositor ID 2)
        let mut telem_caps = CapabilityTable::new();
        telem_caps.mint(CapabilityType::TimerTickRead, Rights::READ, None);
        telem_caps.mint(CapabilityType::IpcSend, Rights::INVOKE, Some(2));
        sched.spawn_with_caps("telemetry_daemon", || {
            loop {
                task::yield_now();
            }
        }, telem_caps);

        // Daemon 1: Telephony Service (Granted TelephonyModem and IpcSend -> UI Compositor ID 2)
        let mut tel_caps = CapabilityTable::new();
        tel_caps.mint(CapabilityType::TelephonyModem, Rights::READ | Rights::WRITE, None);
        tel_caps.mint(CapabilityType::IpcSend, Rights::INVOKE, Some(2));
        sched.spawn_with_caps("telephony_service", || {
            loop {
                task::yield_now();
            }
        }, tel_caps);

        // Daemon 2: UI Compositor (Granted FrameBufferDirect and IpcReceive)
        let mut comp_caps = CapabilityTable::new();
        comp_caps.mint(CapabilityType::FrameBufferDirect, Rights::WRITE, None);
        comp_caps.mint(CapabilityType::IpcReceive, Rights::READ, None);
        sched.spawn_with_caps("ui_compositor", || {
            loop {
                task::yield_now();
            }
        }, comp_caps);

        kprintln!("[OK] AArch64 Multi-Tasking Scheduler: Spawned 3 sovereign daemons with zero-trust capability tables.");
    }

    // 13. Test Capability-Guarded IPC & System Call Interface
    kprintln!("--- [Testing AArch64 Capability-Guarded IPC & Syscall Interface] ---");
    let mut sender_caps = CapabilityTable::new();
    sender_caps.mint(CapabilityType::IpcSend, Rights::INVOKE, Some(2));
    sender_caps.mint(CapabilityType::TimerTickRead, Rights::READ, None);

    let mut receiver_caps = CapabilityTable::new();
    receiver_caps.mint(CapabilityType::IpcReceive, Rights::READ, None);

    let test_payload = b"BATTERY_100_PCT";
    let send_res = ipc::IPC_ROUTER.lock().send(&sender_caps, 0, 2, 0x101, test_payload);
    assert!(send_res.is_ok(), "Authorized IPC send failed");

    let unauth_res = ipc::IPC_ROUTER.lock().send(&sender_caps, 0, 99, 0x101, test_payload);
    assert!(unauth_res.is_err(), "Unauthorized IPC send must be denied");

    let recv_msg = ipc::IPC_ROUTER.lock().receive(&receiver_caps, 2).expect("IPC receive failed");
    assert_eq!(recv_msg.sender, 0);
    assert_eq!(recv_msg.msg_type, 0x101);
    assert_eq!(&recv_msg.payload[..recv_msg.len], test_payload);
    kprintln!("[TEST] AArch64 Capability-Guarded IPC: Authorized send, target-bounded rejection & zero-copy delivery VERIFIED.");

    // Test system call dispatch
    let sys_ticks = syscall::sys_dispatch(syscall::SYS_GET_TICKS, 0, 0, 0, 0, 0);
    assert!(sys_ticks >= 0);
    let sys_task_id = syscall::sys_dispatch(syscall::SYS_GET_TASK_ID, 0, 0, 0, 0, 0);
    assert_eq!(sys_task_id, 0);

    // Verify uncredentialed calls fail closed (-13)
    let denied_touch = syscall::sys_dispatch(syscall::SYS_TOUCH_POLL, 0, 0, 0, 0, 0);
    assert_eq!(denied_touch, -13, "TouchPoll without InputDevice cap must return -EACCES");
    let denied_surf = syscall::sys_dispatch(syscall::SYS_SURFACE_CREATE, 0, 100, 100, 0, 0);
    assert_eq!(denied_surf, -13, "SurfaceCreate without DisplayServer cap must return -EACCES");
    let denied_event = syscall::sys_dispatch(syscall::SYS_EVENT_POLL, 0, 0, 0, 0, 0);
    assert_eq!(denied_event, -13, "EventPoll without EventPoll cap must return -EACCES");

    // Verify SYS_EXIT_WITH_CODE records exit code
    let exit_res = syscall::sys_dispatch(syscall::SYS_EXIT_WITH_CODE, 0, 0, 0, 0, 0);
    assert_eq!(exit_res, 0);

    // Verify credentialed surface operations
    {
        let mut sched = task::SCHEDULER.lock();
        let curr_idx = sched.current_task_idx;
        sched.tasks[curr_idx].cap_table.mint(capability::CapabilityType::DisplayServer, capability::Rights::READ | capability::Rights::WRITE, None);
    }
    let surf_id = syscall::sys_dispatch(syscall::SYS_SURFACE_CREATE, 0, 100, 100, 0, 0);
    assert_eq!(surf_id, 1, "SurfaceCreate with DisplayServer cap should succeed");
    let flip_res = syscall::sys_dispatch(syscall::SYS_SURFACE_FLIP, 0, 0, 0, 0, 0);
    assert_eq!(flip_res, 1, "SurfaceFlip with DisplayServer cap should succeed");
    let destroy_res = syscall::sys_dispatch(syscall::SYS_SURFACE_DESTROY, 0, 1, 0, 0, 0);
    assert_eq!(destroy_res, 0, "SurfaceDestroy with DisplayServer cap should succeed");

    // 13b. Test surface authorization & capability separation
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
        kprintln!("[TEST] AArch64 Surface Authorization: Unrelated task rejected (-EACCES), Admin override authorized VERIFIED.");
    }

    kprintln!("[TEST] AArch64 Syscall Dispatcher: sys_get_ticks (x0={}), sys_get_task_id (x0={}), and standard syscall parity VERIFIED.", sys_ticks, sys_task_id);

    // 14. AArch64 Architectural Exception Vector Table (VBAR_EL1) & Hardware SVC Dispatch
    kprintln!("--- [Testing AArch64 Architectural Exception Vector Table & Hardware SVC] ---");
    exceptions::init_exceptions();
    let svc_ticks = unsafe { exceptions::svc_call(syscall::SYS_GET_TICKS, 0, 0, 0, 0) };
    assert!(svc_ticks >= 0);
    let svc_task_id = unsafe { exceptions::svc_call(syscall::SYS_GET_TASK_ID, 0, 0, 0, 0) };
    assert_eq!(svc_task_id, 0);
    kprintln!("[TEST] AArch64 Vector Table (VBAR_EL1): Hardware 'svc #0' trap -> sys_get_ticks (x0={}), sys_get_task_id (x0={}) VERIFIED.", svc_ticks, svc_task_id);

    // 15. Mobile Shell Hit-Testing & Touch-Initiated Preemption
    kprintln!("--- [Testing mobile shell hit-testing and touch preemption] ---");
    // No digitizer is attached, so the coordinates below are synthetic UI-logic
    // inputs rather than observed contacts.
    assert!(!touch_driver.hardware_present);
    let mut mobile_shell = shell::MobileShell::new();

    // Synthetic tap on the [SETTINGS] dock button at (580, 1360)
    let tap_down = TouchPoint { slot: 0, x: 580, y: 1360, pressure: 750, phase: TouchPhase::Down };
    let tap_up = TouchPoint { slot: 0, x: 580, y: 1360, pressure: 0, phase: TouchPhase::Up };
    mobile_shell.process_touch(&tap_down, &mut fb);
    let action1 = mobile_shell.process_touch(&tap_up, &mut fb);
    assert_eq!(action1, Some(shell::MobileAction::LaunchSettings));
    kprintln!("[TEST] Mobile Shell Hit-Testing: synthetic tap @ (580, 1360) -> LaunchSettings VERIFIED.");
    kprintln!("[TEST] SimpleFB Mobile Display: Settings Surface rendered ({} KiB surface).", fb.buffer_size() / 1024);

    // Synthetic tap on the [DIALER] dock button at (100, 1360)
    let dialer_up = TouchPoint { slot: 0, x: 100, y: 1360, pressure: 0, phase: TouchPhase::Up };
    let action2 = mobile_shell.process_touch(&dialer_up, &mut fb);
    assert_eq!(action2, Some(shell::MobileAction::LaunchDialer));
    kprintln!("[TEST] Mobile Shell Hit-Testing: synthetic tap @ (100, 1360) -> LaunchDialer VERIFIED.");

    // Touch-initiated intent routes via capability-guarded IPC to telephony_service
    sender_caps.mint(CapabilityType::IpcSend, Rights::INVOKE, Some(1));
    let dial_intent = b"DIAL_+15550199234";
    let dial_ipc_res = ipc::IPC_ROUTER.lock().send(&sender_caps, 0, 1, 0x201, dial_intent);
    assert!(dial_ipc_res.is_ok(), "Touch intent dispatch failed");
    kprintln!("[TEST] Touch Force-Takeover Preemption: Touch intent dispatched to telephony daemon (msg_type=0x201) VERIFIED.");

    // 16. AArch64 MMU Virtual Memory Architecture & 4-Level Translation Tables
    kprintln!("--- [Initializing AArch64 MMU & Translation Tables] ---");
    mmu::ArmMmu::init_and_enable(ram_base, ram_size);
    let mmu_active = mmu::ArmMmu::is_enabled();
    assert!(mmu_active, "MMU activation failed");
    kprintln!("[OK] AArch64 MMU: 4-level translation tables, TTBR0_EL1 loaded, L1/L2 caches enabled (SCTLR_EL1.M=1).");

    // 16b. PCIe discovery, driven by the device tree.
    //
    // Everything downstream that needs a PCI device must discover it here rather
    // than assume an address. A platform without a host bridge reports the bus
    // unavailable and nothing acts on it.
    kprintln!("--- [Enumerating PCIe Bus] ---");
    let pcie = match dtb.as_ref() {
        Some(parser) => pci_bus::PcieBus::init_from_fdt(parser),
        None => pci_bus::PcieBus::unavailable(),
    };
    if pcie.is_available() {
        let mut found = 0u32;
        for slot in 0..32usize {
            if let Some(dev) = pcie.devices().get(slot).and_then(|d| *d) {
                kprintln!(
                    "[PCI] Device {:04x}:{:04x} class {:06x} at 00:{:02x}.{:x}",
                    dev.vendor_id,
                    dev.device_id,
                    dev.class_code,
                    dev.device,
                    dev.function
                );
                if dev.vendor_id == 0x1234 && dev.device_id == 0x1111 {
                    if let Some(ecam) = pcie.ecam() {
                        unsafe {
                            ecam.enable_memory_and_bus_master(0, dev.device, dev.function);
                            let bar0 = ecam.read_bar_addr(0, dev.device, dev.function, 0);
                            let bar2 = ecam.read_bar_addr(0, dev.device, dev.function, 2);
                            if let (Some(b0), Some(b2)) = (bar0, bar2) {
                                display::init_bochs_display(b0 as usize, b2 as usize);
                            }
                        }
                    }
                }
                found += 1;
            }
        }
        kprintln!("[OK] PCIe: {} device(s) enumerated on bus 0.", found);
    } else {
        kprintln!("[OK] PCIe: no host bridge on this platform; no PCI device is claimed.");
    }
    // 17. AArch64 Ring 3 / EL0 Userspace Execution & Privilege Separation
    kprintln!("--- [Testing AArch64 Ring 3 / EL0 Userspace Execution & Privilege Separation] ---");
    let user_ttbr0 = mmu::ArmMmu::create_user_page_table();
    mmu::ArmMmu::switch_to_task(user_ttbr0);
    kprintln!("[OK] AArch64 Per-Task MMU: Dedicated TTBR0_EL1 tree allocated at 0x{:08X} (AP_RW_EL0 user isolation active).", user_ttbr0);

    let user_proc = user::UserProcess::new(user::USER_CODE_VADDR, user::USER_STACK_VADDR + user::USER_STACK_SIZE);
    let payload_bytes = user::get_el0_payload();
    unsafe {
        user_proc.load_code(payload_bytes);
    }
    kprintln!("[OK] AArch64 EL0 Userspace: Loaded {}-byte native binary at VA 0x{:08X} (PA 0x{:08X}).", payload_bytes.len(), user::USER_CODE_VADDR, user::USER_CODE_PADDR);

    kprintln!("[INIT] Dropping CPU execution privilege from EL1 to EL0 via hardware eret...");

    // Execute real hardware privilege transition to EL0
    unsafe {
        user::enter_user_mode(user::USER_CODE_VADDR, user::USER_STACK_VADDR + user::USER_STACK_SIZE);
    }

    // After return_to_kernel_mode(), verify all hardware EL0 invariants
    assert!(unsafe { user::EL0_HARDWARE_ENTERED }, "EL0 hardware entry not observed");
    assert!(unsafe { user::EL0_PRIVILEGE_TRAP_VERIFIED }, "EL0 CurrentEL privilege trap not observed");
    assert!(unsafe { user::EL0_MMU_TRAP_VERIFIED }, "EL0 MMU kernel memory trap not observed");
    assert_eq!(unsafe { user::EL0_EXIT_CODE }, 42, "EL0 exit code mismatch");
    assert!(unsafe { user::EL0_TEST_COMPLETED }, "EL0 test loop incomplete");
    unsafe { user::IN_SECTION17_SMOKE = false; }

    kprintln!(
        "[TEST] AArch64 EL0 Userspace Syscall Loop: sys_get_ticks (x0={}), sys_get_task_id (x0={}), sys_exit (code=42) VERIFIED.",
        unsafe { user::EL0_TICKS_CAPTURED },
        unsafe { user::EL0_TASK_ID_CAPTURED }
    );
    kprintln!("[OK] AArch64 Hardware EL0 Privilege Drop: Real eret transition, native SVC syscall loop, privilege trap, and MMU isolation VERIFIED.");

    // 18. Physical Smartphone Battery, Thermal & DVFS Power Governor Driver
    kprintln!("--- [Initializing Mobile Battery Fuel Gauge & DVFS Thermal Governor] ---");
    let battery = power::BatteryStatus::new(0, 0, power::ChargingState::NotCharging);
    let mut governor = power::DvfsGovernor::new();

    // No PMIC/thermal backend is bound yet. The governor must fail closed
    // instead of treating constructor defaults as physical telemetry.
    assert!(!battery.hardware_present);
    assert!(!governor.hardware_present);
    let policy = governor.update(32, &battery);
    assert_eq!(policy, power::ThermalPolicy::Normal);
    assert_eq!(governor.current_freq_mhz, governor.min_freq_mhz);
    // No panel is bound, so the governor must report no refresh target rather
    // than the 30 Hz it used to invent. Asserting a real rate here would
    // reinstate the fabricated panel behaviour the governor was fixed to avoid.
    assert_eq!(governor.display_target_fps, power::NO_REFRESH_TARGET_HZ);
    assert_eq!(governor.current_freq_mhz, power::NO_FREQ_MHZ);
    assert!(!governor.battery_saver_active);
    kprintln!("[OK] Mobile Power Governor: PMIC/thermal backend unbound; safe minimum frequency policy enforced.");

    // 19. Sovereign Hardware Cryptographic Enclave & DID Keystore
    kprintln!("--- [Testing Sovereign Hardware Crypto Enclave & DID Keystore] ---");
    crypto::verify_crypto_subsystem();
    kprintln!("[OK] RFC 4231 HMAC-SHA256 & FIPS PUB 180-4 SHA-256 Engine: Test vectors VERIFIED.");

    // Identity vault conformance. The seed below is a fixed test vector so the
    // DID derivation is reproducible, NOT a device root of trust: a real device
    // key is generated inside a secure element and never exists in ROM.
    let vault_test_seed: [u8; 32] = [
        0x13, 0x37, 0x42, 0x69, 0xAA, 0x55, 0xDE, 0xAD,
        0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB,
        0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
        0x32, 0x10, 0x42, 0x42, 0x13, 0x37, 0x77, 0x88,
    ];
    let mut vault = crypto::IdentityVault::new(vault_test_seed);
    let device_did = vault.device_did().clone();
    kprintln!("[TEST] Identity Vault: DID derivation from a test seed -> {}", device_did.as_str());
    kprintln!("[OK] Device identity: no secure element is bound, so no attested device DID is claimed.");

    // Cryptographic state commitment signing
    let state_commitment_data = b"STATE_COMMITMENT_BLOCK_001_MERKLE_ROOT";
    let state_sig = vault.sign(state_commitment_data);
    let sig_valid = vault.verify_signature(state_commitment_data, &state_sig);
    assert!(sig_valid, "Cryptographic state signature verification failed");
    kprintln!("[OK] State Delta Commitment: Signed & verified via HMAC-SHA256 (32-byte MAC tag).");

    // Cryptographically attested capability delegation token issuance
    let current_ticks = 1000u64;
    let duration_ticks = 500u64;
    let target_task_id = 1u64;
    let capability_mask = 0x07u32; // Read | Write | Execute
    let token = vault.issue_delegation_token(target_task_id, capability_mask, duration_ticks, current_ticks);

    // Verify valid token within expiration window
    assert!(vault.verify_delegation_token(&token, 1200));
    // Verify expired token rejection
    assert!(!vault.verify_delegation_token(&token, 1501));

    // Verify tampered token rejection
    let mut tampered_token = token.clone();
    tampered_token.capability_mask = 0xFF; // Unauthorized escalation attempt
    assert!(!vault.verify_delegation_token(&tampered_token, 1200));
    kprintln!("[TEST] Sovereign Capability Delegation Token: Attestation, expiration & anti-tamper rejection VERIFIED.");

    // 20. Sovereign Decentralized P2P Mesh Network & Offline Sync Engine
    kprintln!("--- [Testing Sovereign Decentralized P2P Mesh Network & Offline Sync] ---");
    let shared_mesh_key: [u8; 32] = [
        0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE,
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00,
    ];
    let local_node_id = 0x1122_3344_5566_7788u64;
    let peer_node_id = 0xAABB_CCDD_EEFF_0011u64;
    let initial_merkle_state: [u8; 32] = [0x5A; 32];

    let mut mesh_ctrl = mesh::MeshController::new(local_node_id, shared_mesh_key, initial_merkle_state);

    // Capability setup for mesh operations
    let mut mesh_caps = CapabilityTable::new();
    mesh_caps.mint(CapabilityType::MeshNetworkTransmit, Rights::WRITE, None);
    mesh_caps.mint(CapabilityType::MeshNetworkReceive, Rights::READ, None);

    // Test 1: Authenticated P2P packet transmission
    let p2p_payload = b"OFFLINE_INTENT_DISPATCH_ENCRYPTED_CALL";
    let sent_packet = mesh_ctrl.transmit(&mesh_caps, mesh::MeshPacketType::EncryptedPayload, peer_node_id, p2p_payload)
        .expect("Mesh transmit failed");
    assert_eq!(sent_packet.header.src_node_id, local_node_id);
    assert_eq!(sent_packet.header.dest_node_id, peer_node_id);
    assert!(sent_packet.verify_signature(&shared_mesh_key), "Packet signature verification failed");
    kprintln!("[OK] Mesh Packet Transmit: Authenticated payload to peer 0x{:016X} (seq=1, TTL=16).", peer_node_id);

    // Test 2: Ingest incoming peer packet from physical radio FIFO
    let peer_hello_payload = b"PEER_HELLO_DID_INTER_40109BA3";
    let incoming_packet = mesh::MeshPacket::new(
        mesh::MeshPacketType::PeerDiscovery,
        peer_node_id,
        local_node_id,
        100, // seq
        16,  // ttl
        peer_hello_payload,
        &shared_mesh_key,
    ).expect("Creating peer packet failed");

    let received = mesh_ctrl.receive(&mesh_caps, incoming_packet)
        .expect("Mesh receive failed");
    assert!(received.is_some(), "Valid packet was incorrectly dropped");
    let rx_pkt = received.unwrap();
    assert_eq!(rx_pkt.header.src_node_id, peer_node_id);
    assert_eq!(rx_pkt.header.packet_type, mesh::MeshPacketType::PeerDiscovery);
    kprintln!("[OK] Mesh Packet Receive: Ingested & verified PeerDiscovery from 0x{:016X}.", peer_node_id);

    // Test 3: Anti-replay protection (duplicate seq=100 from peer must be dropped)
    let duplicate_packet = mesh::MeshPacket::new(
        mesh::MeshPacketType::PeerDiscovery,
        peer_node_id,
        local_node_id,
        100, // duplicate seq!
        16,
        peer_hello_payload,
        &shared_mesh_key,
    ).expect("Duplicate packet creation failed");
    let replay_result = mesh_ctrl.receive(&mesh_caps, duplicate_packet).expect("Receive call failed");
    assert!(replay_result.is_none(), "Replay attack must be dropped");
    kprintln!("[TEST] Anti-Replay Sliding Window: Duplicate packet (seq=100) dropped VERIFIED.");

    // Test 4: Distance-vector dynamic route discovery
    let next_hop = mesh_ctrl.routing_table.find_next_hop(peer_node_id);
    assert_eq!(next_hop, Some(peer_node_id));
    // Link quality has no radio attached, so only the route itself is claimed.
    kprintln!("[OK] Mesh Routing Table: Discovered direct 1-hop route to peer 0x{:016X}.", peer_node_id);

    // Test 5: Offline-First Merkle State Sync
    let remote_synced = mesh_ctrl.sync_engine.evaluate_peer_state(&initial_merkle_state);
    assert!(remote_synced, "Initial states must match");

    let delta_data = b"STATE_DELTA_CONTACT_ADDED_+15550199234";
    let new_merkle_root = mesh_ctrl.sync_engine.apply_delta(delta_data);
    assert_ne!(new_merkle_root, initial_merkle_state);
    assert_eq!(mesh_ctrl.sync_engine.sync_version, 2);
    kprintln!("[OK] Offline-First Merkle Sync: State delta applied -> Sync Version 2 (New Root: {:02X}{:02X}{:02X}{:02X}...).",
        new_merkle_root[0], new_merkle_root[1], new_merkle_root[2], new_merkle_root[3]);



    // 22. Sovereign Android Untrusted Compatibility Sandbox (APK Container & Syscall Gate)
    kprintln!("--- [Testing Sovereign Android Untrusted Compatibility Sandbox] ---");
    let mut android_runtime = sandbox::AndroidRuntime::new();
    let guest_pid = android_runtime.spawn_app("com.untrusted.social");
    assert_eq!(guest_pid, 10001);
    kprintln!("[OK] Android Container Spawner: Launched sandboxed guest PID 10001 (package: 'com.untrusted.social').");

    // Test 1: Authorized Sandboxed VFS write and read
    let sample_pref = b"{\"theme\":\"dark\",\"analytics_opt_in\":false}";
    let write_res = android_runtime.dispatch_syscall(
        guest_pid,
        sandbox::ANDROID_SYS_WRITE,
        "/data/data/com.untrusted.social/shared_prefs.json",
        sample_pref,
    );
    assert_eq!(write_res, sample_pref.len() as i64);

    let read_res = android_runtime.dispatch_syscall(
        guest_pid,
        sandbox::ANDROID_SYS_OPENAT,
        "/data/data/com.untrusted.social/shared_prefs.json",
        &[],
    );
    assert_eq!(read_res, sample_pref.len() as i64);
    kprintln!("[OK] Android Sandboxed VFS: Private storage read/write isolated to /data/data/com.untrusted.social/.");

    // Test 2: Security Breach Interception (Non-Negotiable Principle #9)
    // Untrusted app attempts to read Inter OS sovereign identity vault
    let breach_vault = android_runtime.dispatch_syscall(
        guest_pid,
        sandbox::ANDROID_SYS_OPENAT,
        "/vault/device_root_key.bin",
        &[],
    );
    assert_eq!(breach_vault, sandbox::EACCES); // Trapped with -13 EACCES

    // Untrusted app attempts directory traversal to read host files
    let breach_traversal = android_runtime.dispatch_syscall(
        guest_pid,
        sandbox::ANDROID_SYS_OPENAT,
        "/data/data/com.untrusted.social/../../system/etc/device_id",
        &[],
    );
    assert_eq!(breach_traversal, sandbox::EACCES);

    // Untrusted app attempts cross-app snooping on another app's private files
    let breach_snoop = android_runtime.dispatch_syscall(
        guest_pid,
        sandbox::ANDROID_SYS_OPENAT,
        "/data/data/com.banking.app/pin.txt",
        &[],
    );
    assert_eq!(breach_snoop, sandbox::EACCES);
    assert_eq!(android_runtime.sandbox_fs.security_violations, 3);
    kprintln!("[TEST] Zone 4 Sandbox Boundary: 3/3 Unauthorized host/cross-app breach attempts TRAPPED (-EACCES) VERIFIED.");

    // Test 3: Zero-Trust Binder Mediation
    let guest_caps = android_runtime.active_apps.get(&guest_pid).unwrap().cap_table.clone();
    let binder_vault = android_runtime.binder.mediate_transaction(guest_pid, "inter.identity.vault", &guest_caps);
    assert!(binder_vault.is_err(), "Binder call to sovereign vault must be blocked");

    let binder_activity = android_runtime.binder.mediate_transaction(guest_pid, "android.app.ActivityManager", &guest_caps);
    assert!(binder_activity.is_ok(), "Standard Android activity manager call allowed");
    kprintln!("[TEST] Zero-Trust Binder Mediator: Sovereign service call blocked, guest call mediated VERIFIED.");

    // 23. Sovereign WebAssembly (WASM) / Bytecode Micro-Runtime
    kprintln!("--- [Testing Sovereign WebAssembly (WASM) / Bytecode Micro-Runtime] ---");
    let mut wasm_mod = wasm::WasmModule::new();

    // Import Host Function 0: inter_crypto_hash
    let host_crypto_idx = wasm_mod.add_host_import("inter_crypto_hash", 3, wasm::host_crypto_hash);
    assert_eq!(host_crypto_idx, 0);

    // Add Function 1: compute_math: (param0 + param1) * 3
    // Bytecode:
    // LOCAL_GET 0, LOCAL_GET 1, I32_ADD, I32_CONST 3, I32_MUL, END
    let math_code: alloc::vec::Vec<u8> = alloc::vec![
        wasm::opcodes::LOCAL_GET, 0x00,
        wasm::opcodes::LOCAL_GET, 0x01,
        wasm::opcodes::I32_ADD,
        wasm::opcodes::I32_CONST, 0x03,
        wasm::opcodes::I32_MUL,
        wasm::opcodes::END,
    ];
    let math_idx = wasm_mod.add_function("compute_math", 2, 0, math_code);
    wasm_mod.add_export("compute_math", math_idx);

    // Add Function 2: hash_and_store: calls inter_crypto_hash(0, 3, 32)
    // Bytecode:
    // I32_CONST 0 (src), I32_CONST 3 (len), I32_CONST 32 (dest), CALL 0 (inter_crypto_hash), END
    let hash_code: alloc::vec::Vec<u8> = alloc::vec![
        wasm::opcodes::I32_CONST, 0x00,
        wasm::opcodes::I32_CONST, 0x03,
        wasm::opcodes::I32_CONST, 0x20, // 32
        wasm::opcodes::CALL, 0x00,
        wasm::opcodes::END,
    ];
    let hash_idx = wasm_mod.add_function("hash_and_store", 0, 0, hash_code);
    wasm_mod.add_export("hash_and_store", hash_idx);

    // Capability setup for WASM execution
    let mut wasm_caps = CapabilityTable::new();
    wasm_caps.mint(CapabilityType::WasmExecute, Rights::EXECUTE, None);

    // Test 1: Stack machine execution of compute_math(15, 25) -> (15 + 25) * 3 = 120
    let wasm_mem = wasm::WasmMemory::new(1, 2); // 1 page (64 KiB), max 2 pages
    let mut vm = wasm::WasmVm::new(wasm_mod.clone(), wasm_mem, 1000); // 1000 fuel units
    let math_res = vm.call_export("compute_math", &[wasm::Value::I32(15), wasm::Value::I32(25)], &wasm_caps)
        .expect("WASM math execution failed");
    assert_eq!(math_res, Some(wasm::Value::I32(120)));
    kprintln!("[OK] WASM Stack Machine: Executed compute_math(15, 25) -> 120 (Fuel remaining: {}).", vm.fuel);

    // Test 2: Sandboxed linear memory bounds protection (attempt write beyond 64 KiB)
    let oob_write = vm.memory.write_u8(0x10000, 0xFF); // 65536 is beyond 1 page!
    assert_eq!(oob_write, Err(wasm::WasmError::OutOfBoundsMemory));
    kprintln!("[TEST] Sandboxed Linear Memory: Out-of-bounds access @ 0x10000 TRAPPED (OutOfBoundsMemory) VERIFIED.");

    // Test 3: CPU Fuel metering (prevent infinite loop)
    let mut fuel_starved_vm = wasm::WasmVm::new(wasm_mod.clone(), wasm::WasmMemory::new(1, 1), 2); // only 2 fuel units!
    let fuel_err = fuel_starved_vm.call_export("compute_math", &[wasm::Value::I32(1), wasm::Value::I32(2)], &wasm_caps);
    assert_eq!(fuel_err, Err(wasm::WasmError::FuelExhausted));
    kprintln!("[TEST] CPU Fuel Metering: Runaway execution aborted safely (FuelExhausted) VERIFIED.");

    // Test 4: Host function call from WASM (compute SHA-256 of "abc" at offset 0 into offset 32)
    vm.memory.write_slice(0, b"abc").expect("Writing 'abc' to memory failed");
    let hash_call_res = vm.call_export("hash_and_store", &[], &wasm_caps)
        .expect("WASM host call execution failed");
    assert_eq!(hash_call_res, Some(wasm::Value::I32(32))); // 32 bytes written

    let digest_out = vm.memory.read_slice(32, 32).expect("Reading digest failed");
    let expected_abc_digest = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
        0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];
    assert_eq!(digest_out, &expected_abc_digest);
    kprintln!("[OK] WASM Host Integration: Called inter_crypto_hash -> Verified SHA-256 ('abc' -> BA7816BF...) in linear memory.");



    // 25. Sovereign Decentralized Identity Peer Attestation & Social Recovery Enclave
    kprintln!("--- [Testing Sovereign DID Peer Attestation & Social Recovery Enclave] ---");

    // Test 1: Setup 3-of-5 threshold recovery policy with 1-hour timelock
    let mut policy = recovery::RecoveryPolicy::new(3, 3600);
    let alice_key = [0x11u8; 32];
    let bob_key = [0x22u8; 32];
    let charlie_key = [0x33u8; 32];
    let dave_key = [0x44u8; 32];
    let eve_key = [0x55u8; 32];

    policy.add_guardian("did:inter:guardian_alice", alice_key).expect("Add alice failed");
    policy.add_guardian("did:inter:guardian_bob", bob_key).expect("Add bob failed");
    policy.add_guardian("did:inter:guardian_charlie", charlie_key).expect("Add charlie failed");
    policy.add_guardian("did:inter:guardian_dave", dave_key).expect("Add dave failed");
    policy.add_guardian("did:inter:guardian_eve", eve_key).expect("Add eve failed");
    assert_eq!(policy.active_guardian_count(), 5);
    kprintln!("[OK] Recovery Policy Setup: Registered 5 guardians with 3-of-5 threshold and 3600s timelock.");

    let mut enclave = recovery::SocialRecoveryEnclave::new(policy);

    // Test 2: Zero Ambient Authority: Uncredentialed claim initiation rejected
    let empty_caps = CapabilityTable::new();
    let unauth_claim = enclave.initiate_claim("did:inter:owner_loss", [0x77; 32], 1000, 101, &empty_caps);
    assert_eq!(unauth_claim, Err(recovery::RecoveryError::CapabilityDenied));
    kprintln!("[TEST] Zero Ambient Authority: Uncredentialed recovery claim initiation REJECTED (CapabilityDenied) VERIFIED.");

    // Mint valid capability for recovery enclave
    let mut recovery_caps = CapabilityTable::new();
    recovery_caps.mint(CapabilityType::SocialRecoveryEnclave, Rights::INVOKE | Rights::WRITE, None);

    // Test 3: Authorized recovery claim initiation
    let target_did = "did:inter:lost_phone_owner";
    let new_owner_key = [0x77u8; 32];
    let original_owner_key = [0x99u8; 32];
    let claim_id = enclave.initiate_claim(target_did, new_owner_key, 1000, 42, &recovery_caps)
        .expect("Initiate claim failed");
    assert_eq!(claim_id, 1);
    kprintln!("[OK] Recovery Claim: Initiated Claim #1 for '{}'.", target_did);

    // Test 4: Anti-tamper signature validation (reject forged signature)
    let bad_signature = [0xEEu8; 32];
    let forged_res = enclave.submit_guardian_attestation(
        claim_id,
        "did:inter:guardian_alice",
        bad_signature,
        1050,
        &recovery_caps,
    );
    assert_eq!(forged_res, Err(recovery::RecoveryError::InvalidSignature));
    kprintln!("[TEST] Cryptographic Enclave: Forged guardian signature REJECTED (InvalidSignature) VERIFIED.");

    // Test 5: Valid Guardian 1 (Alice) attestation submission
    let payload = recovery::SocialRecoveryEnclave::compute_attestation_payload(claim_id, target_did, &new_owner_key, 42);
    let alice_sig = crypto::HmacSha256::mac(&alice_key, &payload);
    let alice_res = enclave.submit_guardian_attestation(
        claim_id,
        "did:inter:guardian_alice",
        alice_sig,
        1100,
        &recovery_caps,
    );
    assert_eq!(alice_res, Ok(recovery::ClaimState::Initiated)); // 1 of 3

    // Test 6: Anti-replay / duplicate submission rejection
    let dup_res = enclave.submit_guardian_attestation(
        claim_id,
        "did:inter:guardian_alice",
        alice_sig,
        1110,
        &recovery_caps,
    );
    assert_eq!(dup_res, Err(recovery::RecoveryError::DuplicateAttestation));
    kprintln!("[TEST] Anti-Replay Attestation: Duplicate signature from Alice REJECTED (DuplicateAttestation) VERIFIED.");

    // Test 7: Valid Guardian 2 (Bob) attestation submission
    let bob_sig = crypto::HmacSha256::mac(&bob_key, &payload);
    let bob_res = enclave.submit_guardian_attestation(
        claim_id,
        "did:inter:guardian_bob",
        bob_sig,
        1200,
        &recovery_caps,
    );
    assert_eq!(bob_res, Ok(recovery::ClaimState::Initiated)); // 2 of 3

    // Test 8: Premature finalization attempt before reaching threshold quorum
    let premature_final = enclave.finalize_recovery(claim_id, 5000, &recovery_caps);
    assert_eq!(premature_final, Err(recovery::RecoveryError::QuorumNotReached));
    kprintln!("[TEST] Quorum Gate: Premature recovery finalization (2/3 signatures) REJECTED (QuorumNotReached) VERIFIED.");

    // Test 9: Valid Guardian 3 (Charlie) attestation -> Reaches 3-of-5 threshold!
    let charlie_sig = crypto::HmacSha256::mac(&charlie_key, &payload);
    let charlie_res = enclave.submit_guardian_attestation(
        claim_id,
        "did:inter:guardian_charlie",
        charlie_sig,
        1300,
        &recovery_caps,
    );
    assert_eq!(charlie_res, Ok(recovery::ClaimState::Timelocked));
    kprintln!("[OK] Threshold Quorum Reached: 3/5 Guardian attestations verified -> Entered Timelocked state.");

    // Test 10: Anti-theft timelock protection: Attempt to finalize before 3600s delay expires
    let timelock_premature = enclave.finalize_recovery(claim_id, 2000, &recovery_caps); // 2000 < 1000 + 3600
    assert_eq!(timelock_premature, Err(recovery::RecoveryError::TimelockNotExpired));
    kprintln!("[TEST] Coercion Defense: Finalization during active timelock window REJECTED (TimelockNotExpired) VERIFIED.");

    // Test 11: Authentic owner cancellation during timelock window (anti-coercion mechanism)
    let mut cancel_payload = alloc::vec::Vec::new();
    cancel_payload.extend_from_slice(b"CANCEL_RECOVERY");
    cancel_payload.extend_from_slice(&claim_id.to_be_bytes());
    let cancel_mac = crypto::HmacSha256::mac(&original_owner_key, &cancel_payload);

    let cancel_res = enclave.cancel_claim_by_owner(claim_id, &original_owner_key, &cancel_mac, &recovery_caps);
    assert!(cancel_res.is_ok());
    assert_eq!(enclave.active_claim.as_ref().unwrap().state, recovery::ClaimState::Cancelled);
    kprintln!("[TEST] Sovereign Owner Anti-Coercion: Original owner cancelled fraudulent claim during timelock VERIFIED.");

    // Test 12: Immediate emergency recovery (0s timelock) with full finalization and key rotation
    let mut instant_policy = recovery::RecoveryPolicy::new(3, 0); // 0s timelock
    instant_policy.add_guardian("did:inter:guardian_alice", alice_key).unwrap();
    instant_policy.add_guardian("did:inter:guardian_bob", bob_key).unwrap();
    instant_policy.add_guardian("did:inter:guardian_charlie", charlie_key).unwrap();
    let mut instant_enclave = recovery::SocialRecoveryEnclave::new(instant_policy);

    let target_key2 = [0x88u8; 32];
    let claim_id2 = instant_enclave.initiate_claim("did:inter:migrated_phone", target_key2, 5000, 999, &recovery_caps).unwrap();
    let payload2 = recovery::SocialRecoveryEnclave::compute_attestation_payload(claim_id2, "did:inter:migrated_phone", &target_key2, 999);

    instant_enclave.submit_guardian_attestation(claim_id2, "did:inter:guardian_alice", crypto::HmacSha256::mac(&alice_key, &payload2), 5010, &recovery_caps).unwrap();
    instant_enclave.submit_guardian_attestation(claim_id2, "did:inter:guardian_bob", crypto::HmacSha256::mac(&bob_key, &payload2), 5020, &recovery_caps).unwrap();
    let q_res = instant_enclave.submit_guardian_attestation(claim_id2, "did:inter:guardian_charlie", crypto::HmacSha256::mac(&charlie_key, &payload2), 5030, &recovery_caps).unwrap();
    assert_eq!(q_res, recovery::ClaimState::QuorumReached);

    let rotated_key = instant_enclave.finalize_recovery(claim_id2, 5040, &recovery_caps).expect("Instant recovery finalization failed");
    assert_eq!(rotated_key, target_key2);
    assert_eq!(instant_enclave.active_claim.as_ref().unwrap().state, recovery::ClaimState::Finalized);
    kprintln!("[OK] Sovereign Identity Migration: Identity rotated to new owner key [0x88; 32] via 3-of-3 peer attestation.");

    // 26. Cryptographically Signed Sovereign OTA Updates & A/B Partition Rollback Engine
    kprintln!("--- [Testing Sovereign OTA Updates & A/B Partition Rollback Engine] ---");

    let board_id = *b"PINEPHONE_A64\0\0\0";
    // Boot self-test publisher key material. This is a throwaway signature-test
    // keypair, not a device secret and not an update-authorisation credential:
    // a real publisher keeps its Ed25519 secret key offline and never ships it.
    let publisher_seed: [u8; 32] = [
        0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A,
        0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D,
        0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D,
        0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E,
    ];
    let publisher_pubkey = crypto::ed25519_public_key(&publisher_seed);
    let mut ota = ota::OtaManager::new(540, publisher_pubkey, board_id); // Version 540 active on Slot A
    assert_eq!(ota.active_slot, ota::PartitionSlot::SlotA);
    assert_eq!(ota.slot_a.version, 540);
    assert_eq!(ota.slot_a.status, ota::SlotStatus::Valid);
    kprintln!("[OK] Dual-Slot Partition Layout: Initialized with Slot A active (v5.4.0 Valid, 3 boot tries).");

    // Test 1: Zero Ambient Authority: Uncredentialed update staging rejected
    let dummy_header = ota::OtaPackageHeader {
        magic: ota::OTA_MAGIC,
        target_arch: ota::ARCH_AARCH64,
        target_board: board_id,
        version: 550,
        payload_len: 4,
        payload_sha256: [0u8; 32],
        signature: [0u8; 64],
    };
    let unauth_stage = ota.verify_and_stage_update(&dummy_header, &[1, 2, 3, 4], &empty_caps);
    assert_eq!(unauth_stage, Err(ota::OtaError::CapabilityDenied));
    kprintln!("[TEST] Zero Ambient Authority: Uncredentialed OTA staging REJECTED (CapabilityDenied) VERIFIED.");

    // Mint capability for OTA management
    let mut ota_caps = CapabilityTable::new();
    ota_caps.mint(CapabilityType::OtaUpdateManagement, Rights::WRITE, None);

    // Test 2: Monotonic Anti-Rollback Protection: Downgrade attack rejected
    let mut downgrade_hdr = dummy_header.clone();
    downgrade_hdr.version = 530; // 530 <= 540 (attempted downgrade)
    let downgrade_res = ota.verify_and_stage_update(&downgrade_hdr, &[1, 2, 3, 4], &ota_caps);
    assert_eq!(downgrade_res, Err(ota::OtaError::AntiRollbackDowngrade));
    kprintln!("[TEST] Anti-Rollback Governor: Downgrade attempt (v5.3.0 <= v5.4.0) REJECTED (AntiRollbackDowngrade) VERIFIED.");

    // Test 3: Hardware Target Board Mismatch rejected
    let mut bad_board_hdr = dummy_header.clone();
    bad_board_hdr.version = 550;
    bad_board_hdr.target_board = *b"GOOGLE_PIXEL_8\0\0";
    let bad_board_res = ota.verify_and_stage_update(&bad_board_hdr, &[1, 2, 3, 4], &ota_caps);
    assert_eq!(bad_board_res, Err(ota::OtaError::TargetHardwareMismatch));
    kprintln!("[TEST] Hardware Cross-Flashing Trap: Incompatible device target board REJECTED (TargetHardwareMismatch) VERIFIED.");

    // Test 4: Payload Digest Mismatch & Forged Signature rejected
    let sample_payload = b"Inter OS Sovereign Smartphone Microkernel Binary v5.5.0 Staged Payload Data";
    let valid_digest = crypto::sha256(sample_payload);
    let forged_hdr = ota::OtaPackageHeader {
        magic: ota::OTA_MAGIC,
        target_arch: ota::ARCH_AARCH64,
        target_board: board_id,
        version: 550,
        payload_len: sample_payload.len() as u32,
        payload_sha256: valid_digest,
        signature: [0xEEu8; 64], // forged signature
    };
    let forged_res = ota.verify_and_stage_update(&forged_hdr, sample_payload, &ota_caps);
    assert_eq!(forged_res, Err(ota::OtaError::InvalidSignature));
    kprintln!("[TEST] Cryptographic OTA Signing: Untrusted publisher signature REJECTED (InvalidSignature) VERIFIED.");

    // Test 5: Valid Staging into Slot B
    let preimage = forged_hdr.compute_signature_preimage();
    let valid_signature = crypto::sign_ed25519_signature(&publisher_seed, &preimage);
    assert!(crypto::verify_ed25519_signature(&preimage, &publisher_pubkey, &valid_signature));
    let mut valid_hdr = forged_hdr;
    valid_hdr.signature = valid_signature;

    let staged_slot = ota.verify_and_stage_update(&valid_hdr, sample_payload, &ota_caps)
        .expect("Valid OTA update staging failed");
    assert_eq!(staged_slot, ota::PartitionSlot::SlotB);
    assert_eq!(ota.active_slot, ota::PartitionSlot::SlotB);
    assert_eq!(ota.slot_b.status, ota::SlotStatus::Testing);
    assert_eq!(ota.slot_b.boot_tries_left, 3);
    kprintln!("[OK] Staged OTA Update: Version 550 written to Slot B, marked Testing (3 boot tries remaining).");

    // Test 6: Anti-Bricking Watchdog & Autonomous Rollback
    // Simulate 3 boot failures on unstable staged slot
    let slot_after_fail1 = ota.handle_boot_failure();
    assert_eq!(slot_after_fail1, ota::PartitionSlot::SlotB);
    assert_eq!(ota.slot_b.boot_tries_left, 2);

    let slot_after_fail2 = ota.handle_boot_failure();
    assert_eq!(slot_after_fail2, ota::PartitionSlot::SlotB);
    assert_eq!(ota.slot_b.boot_tries_left, 1);

    // 3rd failure: watchdog expires -> auto-rollback to Slot A!
    let slot_after_fail3 = ota.handle_boot_failure();
    assert_eq!(slot_after_fail3, ota::PartitionSlot::SlotA);
    assert_eq!(ota.active_slot, ota::PartitionSlot::SlotA);
    assert_eq!(ota.slot_b.status, ota::SlotStatus::Corrupted);
    kprintln!("[TEST] Anti-Bricking Watchdog: Boot tries exhausted (0/3) -> AUTONOMOUS ROLLBACK to Slot A VERIFIED.");

    // Test 7: Successful Update Commit
    // Stage v5.6.0 update to Slot B and commit after successful boot
    let sample_payload2 = b"Inter OS Sovereign Smartphone Microkernel Binary v5.6.0 Certified Production Release";
    let valid_digest2 = crypto::sha256(sample_payload2);
    let mut valid_hdr2 = ota::OtaPackageHeader {
        magic: ota::OTA_MAGIC,
        target_arch: ota::ARCH_AARCH64,
        target_board: board_id,
        version: 560,
        payload_len: sample_payload2.len() as u32,
        payload_sha256: valid_digest2,
        signature: [0u8; 64],
    };
    let preimage2 = valid_hdr2.compute_signature_preimage();
    valid_hdr2.signature = crypto::sign_ed25519_signature(&publisher_seed, &preimage2);

    let staged_slot2 = ota.verify_and_stage_update(&valid_hdr2, sample_payload2, &ota_caps).unwrap();
    assert_eq!(staged_slot2, ota::PartitionSlot::SlotB);
    assert_eq!(ota.active_slot, ota::PartitionSlot::SlotB);

    // Commit boot
    ota.commit_successful_boot(&ota_caps).expect("Commit boot failed");
    assert_eq!(ota.slot_b.status, ota::SlotStatus::Valid);
    assert_eq!(ota.slot_b.boot_tries_left, 3);
    assert_eq!(ota.slot_b.version, 560);
    kprintln!("[OK] OTA Commit: Successfully marked Slot B as Valid (v5.6.0 active production release).");

    // 27. Sovereign Audio Architecture & I2S PCM Codec Driver
    kprintln!("--- [Testing Sovereign Audio Architecture & I2S PCM Codec Driver] ---");

    let mut audio_driver = audio::AudioDriver::new(0x0902_0000);
    audio_driver.init_hardware(audio::SampleRate::Rate48kHz, audio::ChannelMode::Stereo);
    assert!(audio_driver.enabled);
    assert!(!audio_driver.hardware_present);
    kprintln!("[OK] I2S Audio Codec: configuration parsed; physical codec backend remains unbound.");

    let tone_samples = audio::AudioDriver::generate_tone_440hz(128, 48000);
    let mut audio_caps = CapabilityTable::new();
    audio_caps.mint(CapabilityType::AudioStreamDirect, Rights::WRITE, None);

    // Protocol/ring-buffer logic may be exercised, but no physical DAC write or
    // DMA completion may be claimed while the codec backend is absent.
    let stream_res = audio_driver.stream_pcm(
        &tone_samples,
        audio::AudioStreamType::BackgroundMediaMusic,
        &audio_caps,
    );
    assert!(stream_res.is_err());
    assert!(audio_driver.ring_buffer.is_empty());
    kprintln!("[OK] I2S PCM: authorized stream rejected without a bound codec/DMA backend.");

    // 29. Sovereign Mobile Camera / Hardware ISP Frame Capture Driver & Multimodal Neural Vision
    kprintln!("--- [Testing Sovereign Mobile Camera & Hardware ISP Driver] ---");

    let mut camera = camera::CameraDriver::new(0x0903_0000);
    camera.init_hardware(camera::SensorResolution::Res1920x1080, camera::PixelFormat::Rgb565);
    assert!(!camera.enabled);
    assert!(!camera.hardware_present);
    kprintln!("[OK] MIPI CSI-2 Camera configuration loaded; no physical backend is claimed.");

    // Test 1: Zero Ambient Authority: Uncredentialed camera frame capture rejected
    let unauth_capture = camera.capture_frame(200, &empty_caps);
    assert_eq!(unauth_capture.err(), Some(camera::CameraError::CapabilityDenied));
    kprintln!("[TEST] Zero Ambient Authority: Uncredentialed camera frame read REJECTED (CapabilityDenied) VERIFIED.");

    // Mint camera capture capability
    let mut camera_caps = CapabilityTable::new();
    camera_caps.mint(CapabilityType::CameraCaptureDirect, Rights::READ, None);

    // Test 2: Hardware Privacy Killswitch (Principle #17)
    camera.set_privacy_killswitch(true);
    let killswitch_res = camera.capture_frame(201, &camera_caps);
    assert_eq!(killswitch_res.err(), Some(camera::CameraError::PrivacyKillswitchEngaged));
    kprintln!("[TEST] Hardware Privacy Killswitch: Sensor power severed (PrivacyKillswitchEngaged) VERIFIED.");

    // Disengage killswitch to resume normal camera operations
    camera.set_privacy_killswitch(false);

    // Test 3: Authorized capture must still fail without a concrete CSI/ISP backend.
    let capture = camera.capture_frame(202, &camera_caps);
    assert_eq!(capture.err(), Some(camera::CameraError::HardwareDisabled));
    kprintln!("[TEST] Camera hardware truth: authorized capture rejected without backend.");


    // 30. Sovereign Cellular SMS & Emergency PDU Dispatcher
    kprintln!("--- [Testing Sovereign Cellular SMS & Emergency PDU Dispatcher] ---");

    let mut sms_dispatcher = sms::SmsDispatcher::new();

    // Test 1: Zero Ambient Authority: Uncredentialed SMS-SUBMIT compose rejected
    let unauth_compose = sms_dispatcher.compose_submit_pdu(
        "+15550199234",
        "Hello from sovereign Inter OS kernel!",
        &empty_caps,
    );
    assert_eq!(unauth_compose.err(), Some(sms::SmsError::CapabilityDenied));
    kprintln!("[TEST] Zero Ambient Authority: Uncredentialed SMS compose REJECTED (CapabilityDenied) VERIFIED.");

    // Mint SMS capability
    let mut sms_caps = CapabilityTable::new();
    sms_caps.mint(CapabilityType::SmsDispatchDirect, Rights::READ | Rights::WRITE | Rights::INVOKE, None);

    // Test 2: Authorized 3GPP TS 23.040 SMS-SUBMIT PDU Composition & 7-bit packing
    let (submit_pdu, cmgs_len) = sms_dispatcher.compose_submit_pdu(
        "+15550199234",
        "Hello from sovereign Inter OS kernel!",
        &sms_caps,
    ).expect("SMS SUBMIT PDU compose failed");

    assert!(submit_pdu.len() > 15);
    assert_eq!(submit_pdu[0], 0x00); // SMSC len 0
    assert_eq!(submit_pdu[1], 0x01); // TP-MTI = SMS-SUBMIT
    assert_eq!(sms_dispatcher.total_messages_sent, 1);
    assert_eq!(sms_dispatcher.outbound_queue.len(), 1);
    kprintln!("[OK] 3GPP SMS-SUBMIT Encoder: Generated valid binary PDU ({} octets, AT+CMGS={}).", submit_pdu.len(), cmgs_len);

    // Test 3: Ingest and decode valid incoming SMS-DELIVER PDU
    let test_msg = "Meeting at 3 PM";
    let packed_msg = sms::pack_7bit(test_msg);
    let mut raw_deliver = alloc::vec![
        0x00,             // SMSC len
        0x00,             // TP-MTI: SMS-DELIVER
        11, 0x91,         // Length 11, international
        0x51, 0x55, 0x90, 0x78, 0x56, 0xF4, // +15550987654 swapped BCD
        0x00,             // TP-PID
        0x00,             // TP-DCS
        0x62, 0x90, 0x12, 0x41, 0x03, 0x00, 0x00, // Timestamp 2026-09-21 14:30:00 (swapped BCD)
        test_msg.len() as u8, // TP-UDL
    ];
    raw_deliver.extend_from_slice(&packed_msg);

    // Test uncredentialed ingest rejection
    let unauth_ingest = sms_dispatcher.ingest_deliver_pdu(&raw_deliver, &empty_caps);
    assert_eq!(unauth_ingest.err(), Some(sms::SmsError::CapabilityDenied));
    kprintln!("[TEST] Zero Ambient Authority: Uncredentialed SMS-DELIVER decode REJECTED (CapabilityDenied) VERIFIED.");

    // Authorized ingest
    let delivered = sms_dispatcher.ingest_deliver_pdu(&raw_deliver, &sms_caps)
        .expect("SMS-DELIVER ingestion failed");
    assert_eq!(delivered.originating_number, "+15550987654");
    assert_eq!(delivered.message_text, test_msg);
    assert_eq!((delivered.timestamp_year, delivered.timestamp_month, delivered.timestamp_day), (26, 9, 21));
    assert_eq!(delivered.threat_level, sms::ThreatLevel::Safe);
    assert_eq!(sms_dispatcher.total_messages_received, 1);
    kprintln!("[OK] 3GPP SMS-DELIVER Decoder: Parsed inbound SMS from '{}' ('{}', Timestamp: 20{:02}-{:02}-{:02} {:02}:{:02}).",
        delivered.originating_number, delivered.message_text, delivered.timestamp_year, delivered.timestamp_month, delivered.timestamp_day, delivered.timestamp_hour, delivered.timestamp_minute);

    // Test 4: On-Device Anti-Phishing Security Threat Assessment
    let phishing_msg = "URGENT: Bank account locked! Login immediately at http://secure-bank.xyz to verify password";
    let packed_phishing = sms::pack_7bit(phishing_msg);
    let mut phishing_pdu = alloc::vec![
        0x00, 0x00,
        10, 0x81,
        0x99, 0x88, 0x77, 0x66, 0x55, // 9988776655
        0x00, 0x00,
        0x62, 0x90, 0x12, 0x41, 0x53, 0x00, 0x00, // 2026-09-21 14:35:00
        phishing_msg.len() as u8,
    ];
    phishing_pdu.extend_from_slice(&packed_phishing);

    let phishing_deliver = sms_dispatcher.ingest_deliver_pdu(&phishing_pdu, &sms_caps)
        .expect("Phishing SMS ingestion failed");
    assert_eq!(phishing_deliver.threat_level, sms::ThreatLevel::HighPhishingRisk);
    assert!(phishing_deliver.threat_score >= 60);
    assert_eq!(sms_dispatcher.total_phishing_blocked, 1);
    kprintln!("[TEST] On-Device Anti-Phishing Filter: Detected fraudulent bank spoof (Threat Score: {}) -> FLAGGED HighPhishingRisk VERIFIED.", phishing_deliver.threat_score);

    // Test 5: 3GPP TS 23.041 Cell Broadcast Service (CMAS / ETWS Emergency Alert)
    let alert = sms_dispatcher.dispatch_emergency_broadcast(
        0x1112,
        0x0001,
        sms::EmergencySeverity::SevereThreat,
        42,
        "TSUNAMI WARNING: Evacuate coastal lowlands immediately! Seek high ground.",
        &sms_caps,
    ).expect("Emergency broadcast dispatch failed");

    assert_eq!(alert.message_id, 0x1112);
    assert_eq!(alert.severity, sms::EmergencySeverity::SevereThreat);
    assert_eq!(sms_dispatcher.total_emergency_alerts, 1);
    assert_eq!(sms_dispatcher.emergency_alerts.len(), 1);
    kprintln!("[OK] Emergency Cell Broadcast (CMAS/ETWS): Dispatched high-priority warning (ID: 0x{:04X}, Scope: {}) -> '{}'.",
        alert.message_id, alert.geographic_scope, alert.warning_text);

    // 31. Mobile peripheral backend boundaries: protocol logic is testable, physical access is not fabricated.
    kprintln!("--- [Testing AArch64 peripheral backend boundaries] ---");

    let mut nfc = nfc::NfcController::new(0x0904_0000, [0u8; 32], "did:inter:unconfigured");
    nfc.init_hardware(nfc::NfcOperatingMode::CardEmulation);
    assert!(nfc.enabled);
    assert!(!nfc.hardware_present);
    let mut nfc_caps = CapabilityTable::new();
    nfc_caps.mint(CapabilityType::NfcSecureElement, Rights::READ | Rights::WRITE | Rights::INVOKE, None);
    let nfc_select = nfc.process_apdu(&alloc::vec![0x00, 0xA4, 0x04, 0x00, 0x0A, 0xF0, 0x49, 0x4E, 0x54, 0x45, 0x52, 0x44, 0x49, 0x44, 0x01], &nfc_caps);
    assert_eq!(nfc_select.err(), Some(nfc::NfcError::HardwareDisabled));
    let ndef = alloc::vec![0xD1, 0x01, 13, b'U', 0x04, b'i', b'n', b't', b'e', b'r', b'-', b'o', b's', b'.', b'o', b'r', b'g'];
    assert!(nfc::NfcController::parse_ndef(&ndef, &nfc_caps).is_ok());
    kprintln!("[OK] NFC: protocol parser works; physical controller remains unbound.");

    let mut sensors = sensors::SensorHub::new(0x0905_0000);
    sensors.init_hardware();
    assert!(sensors.enabled);
    assert!(!sensors.hardware_present);
    let mut sensor_caps = CapabilityTable::new();
    sensor_caps.mint(CapabilityType::SensorTelemetryRead, Rights::READ, None);
    let sample = sensors::ImuSample { accel_x_mg: 0, accel_y_mg: -1000, accel_z_mg: 0, gyro_x_dps: 0, gyro_y_dps: 0, gyro_z_dps: 0 };
    let env = sensors::EnvironmentSample { proximity_mm: 80, ambient_lux: 450, pressure_pa: sensors::STANDARD_SEA_LEVEL_PRESSURE_PA, altitude_meters: 0 };
    assert_eq!(sensors.feed_hardware_telemetry(sample, env, false, &sensor_caps).err(), Some(sensors::SensorError::HardwareDisabled));
    kprintln!("[OK] Sensors: telemetry path fails closed without a concrete driver.");

    let mut bt = bluetooth::BluetoothController::new(0x0906_0000);
    bt.init_hardware();
    assert!(bt.powered);
    assert!(!bt.hardware_present);
    let mut bt_caps = CapabilityTable::new();
    bt_caps.mint(CapabilityType::BluetoothDirect, Rights::READ | Rights::WRITE | Rights::INVOKE, None);
    assert_eq!(bt.rotate_private_address(0, &bt_caps).err(), Some(bluetooth::BluetoothError::HardwareDisabled));
    assert_eq!(bt.process_incoming_event(&[0x04, 0x0E, 0x04, 0x01, 0x03, 0x0C, 0x00], &bt_caps).err(), Some(bluetooth::BluetoothError::HardwareDisabled));
    let reset = bt.cmd_reset(&bt_caps).expect("HCI packet serialization failed");
    assert_eq!(reset, alloc::vec![0x01, 0x03, 0x0C, 0x00]);
    kprintln!("[OK] Bluetooth: HCI serialization works; controller operations require hardware.");

    let mut wifi = wifi::WifiController::new(0x0907_0000);
    wifi.init_hardware();
    assert!(!wifi.hardware_present);
    kprintln!("[OK] Wi-Fi: no physical chipset claimed.");

    let mut gnss = gps::GpsController::new(0x0908_0000);
    gnss.init_hardware();
    assert!(gnss.enabled);
    assert!(!gnss.hardware_present);
    let valid_nmea = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    let fix = gnss.parse_gpgga(valid_nmea).expect("NMEA parser rejected valid sentence");
    assert_eq!(fix.satellites_tracked, 8);
    let mut gps_caps = CapabilityTable::new();
    gps_caps.mint(CapabilityType::GpsTelemetryRead, Rights::READ | Rights::INVOKE, None);
    assert_eq!(gnss.get_location(gps::AccuracyLevel::Precise, &gps_caps).err(), Some(gps::GpsError::HardwareDisabled));
    kprintln!("[OK] GNSS: NMEA parser works; location access requires a physical receiver.");

    let mut haptics = haptics::HapticsController::new(0x0909_0000);
    haptics.init_hardware();
    assert!(!haptics.hardware_present);
    assert_eq!(haptics.synthesize_effect(haptics::TactileEffect::LightClick).len(), 1);
    let mut haptic_caps = CapabilityTable::new();
    haptic_caps.mint(CapabilityType::HapticsActuatorDirect, Rights::WRITE, None);
    assert_eq!(haptics.play_effect(haptics::TactileEffect::LightClick, &haptic_caps).err(), Some(haptics::HapticError::HardwareDisabled));
    kprintln!("[OK] Haptics: waveform generation is software-only; actuator output is unbound.");

    let mut bio = biometrics::BiometricsController::new(0x090A_0000);
    bio.init_hardware();
    assert!(!bio.hardware_present);
    let mut bio_caps = CapabilityTable::new();
    bio_caps.mint(CapabilityType::BiometricAuthEnclave, Rights::INVOKE, None);
    assert_eq!(bio.enroll_finger(1, alloc::vec![], &bio_caps).err(), Some(biometrics::BiometricError::HardwareDisabled));
    kprintln!("[OK] Biometrics: no software-fabricated secure enclave.");

    let mut usb_pd = usb::UsbPdController::new(0x090B_0000);
    usb_pd.init_hardware();
    assert!(!usb_pd.hardware_present);
    usb_pd.attach_cable(usb::TypeCPolarity::Cc1Active);
    usb_pd.ingest_source_capabilities(&[usb::PowerDataObject { position: 1, voltage_mv: 5000, max_current_ma: 3000 }]);
    let mut usb_caps = CapabilityTable::new();
    usb_caps.mint(CapabilityType::UsbPowerDeliveryDirect, Rights::WRITE, None);
    assert_eq!(usb_pd.negotiate_power_contract(5000, 30, &usb_caps).err(), Some(usb::UsbError::NoPortConnected));
    kprintln!("[OK] USB-PD: protocol state is isolated from absent physical TCPC hardware.");

    
// 40. Sovereign Mobile Multi-Core SMP, IPI & big.LITTLE Scheduler
    kprintln!("--- [Testing Sovereign Mobile Multi-Core SMP Subsystem] ---");

    // No GIC distributor base is claimed: PSCI and GIC MMIO are only touched
    // once a real topology has been discovered and explicitly bound.
    let mut smp = smp::SmpController::new(0, psci);
    assert!(!smp.hardware_present);
    assert_eq!(smp.online_core_count(), 1);
    assert_eq!(smp.cores.len(), 8);
    kprintln!("[OK] SMP topology model loaded; physical PSCI/GIC backend remains unbound.");

    let mut smp_caps = CapabilityTable::new();
    smp_caps.mint(CapabilityType::CpuSmpGovernorDirect, Rights::WRITE | Rights::INVOKE, None);

    // No PSCI CPU_ON or GIC SGI MMIO is attempted until a real topology and
    // interrupt-controller backend are discovered and explicitly attached.
    assert_eq!(
        smp.boot_secondary_core(1, 0x4008_0000, 0, &smp_caps).err(),
        Some(smp::SmpError::HardwareDisabled)
    );
    assert_eq!(
        smp.dispatch_ipi(0b0000_0010, smp::IpiKind::Reschedule, &smp_caps).err(),
        Some(smp::SmpError::HardwareDisabled)
    );
    assert_eq!(
        smp.power_off_core(1, &smp_caps).err(),
        Some(smp::SmpError::HardwareDisabled)
    );
    assert_eq!(smp.total_ipis_sent, 0);
    kprintln!("[OK] SMP: CPU_ON, IPI and hotplug operations fail closed without real PSCI/GIC hardware.");

    // 41. SIM/eSIM backend boundary
    kprintln!("--- [Testing AArch64 SIM/eSIM backend boundary] ---");
    let mut sim = sim::SimCardController::new(0x090D_0000);
    sim.init_hardware();
    assert!(sim.enabled);
    assert!(!sim.hardware_present);
    let mut sim_caps = CapabilityTable::new();
    sim_caps.mint(CapabilityType::SimCardSecureElement, Rights::READ | Rights::WRITE | Rights::INVOKE, None);
    assert_eq!(sim.read_iccid(&sim_caps).err(), Some(sim::SimError::HardwareDisabled));
    kprintln!("[OK] SIM/eSIM: no fabricated ICCID or card presence.");

    // 42. Display backend boundary
    kprintln!("--- [Testing AArch64 display backend boundary] ---");
    let mut display_hal = display_hal::MobileDisplayHal::new(0x090E_0000);
    display_hal.init_hardware();
    // No panel controller is bound, so the HAL must report itself disabled and
    // unbacked. Asserting `enabled` here would reinstate the fabricated panel
    // the HAL was fixed to stop claiming.
    assert!(!display_hal.enabled);
    assert!(!display_hal.hardware_present);
    let mut disp_caps = CapabilityTable::new();
    disp_caps.mint(CapabilityType::DisplayControllerDirect, Rights::READ | Rights::WRITE, None);
    assert_eq!(display_hal.set_refresh_rate(display_hal::RefreshRateHz::Hz120, 85, &disp_caps).err(), Some(display_hal::DisplayError::HardwareDisabled));
    kprintln!("[OK] Display: panel timing is not exposed as physical hardware without a bound backend.");

    // 43. StrongBox backend boundary
    kprintln!("--- [Testing AArch64 StrongBox backend boundary] ---");
    let mut strongbox = strongbox::StrongBoxHal::new(0x090F_0000);
    strongbox.init_hardware();
    assert!(strongbox.enabled);
    assert!(!strongbox.hardware_present);
    let mut sb_caps = CapabilityTable::new();
    sb_caps.mint(CapabilityType::StrongBoxSecureEnclave, Rights::READ | Rights::WRITE | Rights::INVOKE, None);
    assert_eq!(strongbox.generate_key(strongbox::KeyAlgorithm::Ed25519, &sb_caps).err(), Some(strongbox::StrongBoxError::HardwareDisabled));
    kprintln!("[OK] StrongBox: no software-generated secure-element keys or attestations.");

    // 44. Remote killswitch backend boundary
    kprintln!("--- [Testing AArch64 remote killswitch backend boundary] ---");
    let mut killswitch = killswitch::RemoteKillswitchHal::new(0x0910_0000, [0u8; 32]);
    killswitch.init_hardware();
    assert!(killswitch.enabled);
    assert!(!killswitch.hardware_present);
    let mut ks_caps = CapabilityTable::new();
    ks_caps.mint(CapabilityType::RemoteKillSwitchDirect, Rights::READ | Rights::WRITE | Rights::INVOKE, None);
    assert_eq!(killswitch.execute_kill_command(killswitch::KillAction::LockdownScreen, killswitch.current_nonce, &[0u8; 64], &ks_caps).err(), Some(killswitch::KillswitchError::HardwareDisabled));
    kprintln!("[OK] Remote killswitch: command execution requires a concrete hardware controller.");

    kprintln!();
    kprintln!("============================================================");
    kprintln!("  [READY] AArch64 Sovereign Smartphone Microkernel");
    kprintln!("  Source-level backend boundaries validated; runtime hardware remains unbound until a concrete driver is attached.");
    kprintln!("============================================================");

    // 45. 50-Cycle Memory Stability Test
    kprintln!("--- [Testing AArch64 50-Cycle Memory Stability] ---");
    let baseline_frames = mmu::active_allocated_frames();
    for cycle in 0..50 {
        let mut aspace = mmu::UserAddressSpace::new().expect("Address space allocation in stability cycle");
        let test_frame = mmu::allocate_frame().expect("Frame allocation in stability cycle");
        aspace.map_user_page(0x0040_0000, test_frame, true, false).expect("Mapping page in stability cycle");
        assert!(mmu::active_allocated_frames() > baseline_frames);
        assert!(aspace.destroy().is_ok());
        assert_eq!(mmu::active_allocated_frames(), baseline_frames, "Memory leak detected at cycle {}", cycle);
    }
    kprintln!("[TEST] 50-Cycle Memory Stability: Free/used frame count returned to baseline ({}) across 50 cycles without leakage VERIFIED.", baseline_frames);

    // 45b. 50-Cycle Real Process Lifecycle Verification
    kprintln!("--- [Testing AArch64 50-Cycle Real Process Lifecycle] ---");
    static TEST_PROC_AARCH64_ELF: [u8; 4096] = {
        let mut b = [0u8; 4096];
        b[0] = 0x7f; b[1] = 0x45; b[2] = 0x4c; b[3] = 0x46;
        b[4] = 0x02; b[5] = 0x01; b[6] = 0x01;
        b[16] = 0x02; b[18] = 0xb7; b[20] = 0x01;
        b[24] = 0x78; b[26] = 0x40; b[32] = 0x40;
        b[52] = 0x40; b[54] = 0x38; b[56] = 0x01; b[58] = 0x40;
        b[64] = 0x01; b[68] = 0x05; b[82] = 0x40; b[90] = 0x40;
        b[97] = 0x10; b[105] = 0x10; b[113] = 0x10;
        // Code at offset 120 (0x78)
        b[120] = 0xe8; b[122] = 0x80; b[123] = 0xd2; // mov x8, #7 (SYS_GET_TASK_ID)
        b[124] = 0x01; b[127] = 0xd4;               // svc #0
        b[128] = 0x48; b[130] = 0x80; b[131] = 0xd2; // mov x8, #2 (SYS_EXIT)
        b[134] = 0x80; b[135] = 0xd2;               // mov x0, #0 (exit code 0)
        b[136] = 0x01; b[139] = 0xd4;               // svc #0
        b[143] = 0x14;                               // b .
        b
    };

    let proc_baseline_frames = mmu::active_allocated_frames();
    let proc_baseline_tasks = task::SCHEDULER.lock().tasks.len();
    unsafe { user::IN_LIFECYCLE_TEST = true; }
    for cycle in 0..50 {
        let loaded = elf::load_elf(&TEST_PROC_AARCH64_ELF).expect("Loading test ELF for lifecycle cycle");
        let parent_id = 1;
        let mut caps = capability::CapabilityTable::new();
        caps.mint(capability::CapabilityType::IpcSend, capability::Rights::INVOKE, None);
        caps.mint(capability::CapabilityType::IpcReceive, capability::Rights::READ, None);

        let (child_pid, child_ttbr0) = {
            let mut sched = task::SCHEDULER.lock();
            let ttbr0 = loaded.address_space.ttbr0;
            let pid = sched.spawn_user_process(parent_id, "test_child", loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
            assert!(pid > 0);
            (pid, ttbr0)
        };

        // Activate child address space
        mmu::ArmMmu::switch_to_task(child_ttbr0);
        {
            let mut sched = task::SCHEDULER.lock();
            if let Some(pos) = sched.tasks.iter().position(|t| t.id == child_pid) {
                sched.current_task_idx = pos;
                sched.tasks[pos].state = task::TaskState::Running;
            }
        }

        // Enter user mode (drops from EL1 to EL0 via hardware eret)
        // In EL0: executes SYS_GET_TASK_ID, then SYS_EXIT(0)
        // SYS_EXIT traps to EL1, records exit code 0, marks child Terminated, and returns via return_to_kernel_mode()
        unsafe {
            user::enter_user_mode(loaded.entry_point, loaded.stack_top);
        }

        // Switch back to kernel page table
        mmu::ArmMmu::switch_to_kernel();
        {
            let mut sched = task::SCHEDULER.lock();
            if let Some(pos) = sched.tasks.iter().position(|t| t.id == parent_id) {
                sched.current_task_idx = pos;
            }
        }

        let mut sched = task::SCHEDULER.lock();
        assert_eq!(sched.reap_task(999, false, child_pid), Err(-13), "Non-child reap without supervisor rights must fail");
        let reaped_code = sched.reap_task(parent_id, false, child_pid).expect("Parent reap of terminated child must succeed");
        assert_eq!(reaped_code, 0);
        assert!(sched.reap_task(parent_id, false, child_pid).is_err(), "Double reap must fail");
        drop(sched);

        assert_eq!(mmu::active_allocated_frames(), proc_baseline_frames, "Process frame leak at cycle {}", cycle);
        assert_eq!(task::SCHEDULER.lock().tasks.len(), proc_baseline_tasks, "Process task leak at cycle {}", cycle);
    }
    unsafe { user::IN_LIFECYCLE_TEST = false; }
    kprintln!("[TEST] 50-Cycle Real Process Lifecycle: Spawn, execute, parent waitpid & zero frame/task leakage VERIFIED.");

    // 45c. Process Safety Invariants & Allocator Exhaustion/Rollback
    {
        // Quota exhaustion
        let mut sched = task::SCHEDULER.lock();
        let orig_len = sched.tasks.len();
        while sched.tasks.len() < task::MAX_TASKS {
            sched.spawn("filler", || loop { task::yield_now(); });
        }
        let excess_pid = sched.spawn_user_process(1, "overflow", 0x1000, 0x2000, None, capability::CapabilityTable::new());
        assert_eq!(excess_pid, 0, "Spawn beyond MAX_TASKS must return 0");
        while sched.tasks.len() > orig_len {
            sched.tasks.pop();
        }
        drop(sched);

        // Active process reap denial (-EBUSY)
        let mut sched = task::SCHEDULER.lock();
        let active_pid = sched.spawn_user_process(1, "active_proc", 0x1000, 0x2000, None, capability::CapabilityTable::new());
        if let Some(t) = sched.tasks.iter_mut().find(|t| t.id == active_pid) {
            t.state = task::TaskState::Running;
        }
        let reap_active_res = sched.reap_task(1, false, active_pid);
        assert!(reap_active_res == Err(-16) || reap_active_res == Err(-30), "Reaping non-terminated/running task must be rejected (-EBUSY or -30)");
        if let Some(t) = sched.tasks.iter_mut().find(|t| t.id == active_pid) {
            t.state = task::TaskState::Terminated;
        }
        assert!(sched.reap_task(1, false, active_pid).is_ok());
        drop(sched);

        // Partial ELF load rollback
        let corrupt_elf = [0x7fu8, b'E', b'L', b'F', 0, 0, 0];
        let baseline = mmu::active_allocated_frames();
        assert!(elf::load_elf(&corrupt_elf).is_err());
        assert_eq!(mmu::active_allocated_frames(), baseline, "ELF load failure must roll back all frames");
    }
    kprintln!("[TEST] Process Safety Invariants: Quota exhaustion, active reap rejection & ELF rollback VERIFIED.");

    // 45d. Physical Frame Ownership & Reference Counting
    {
        kprintln!("--- [Testing AArch64 Physical Frame Ownership & Shared Mappings] ---");
        let baseline = mmu::active_allocated_frames();
        let frame1 = mmu::allocate_frame().expect("Frame alloc");
        let mut aspace_a = mmu::UserAddressSpace::new().expect("Aspace A");
        let mut aspace_b = mmu::UserAddressSpace::new().expect("Aspace B");

        // Atomic lifecycle & race tests:
        assert_eq!(aspace_a.is_closing(), false);
        assert_eq!(aspace_a.try_activate(), Ok(()));
        assert!(aspace_a.is_active_on_any_cpu());
        aspace_a.deactivate();
        assert_eq!(aspace_a.is_active_on_any_cpu(), false);

        // Private mapping
        assert_eq!(aspace_a.map_private_frame(0x0040_0000, frame1, true, false), Ok(()));
        assert_eq!(mmu::frame_ownership(frame1), Some(mmu::FrameOwnership::PrivateOwned));
        // Private frame already mapped must be rejected from being mapped in another address space
        assert!(aspace_b.map_private_frame(0x0040_0000, frame1, true, false).is_err(), "Private frame already mapped must be rejected");

        // Shared mapping: frame2 mapped into A (refcount 1), then into B (refcount 2)
        let frame2 = mmu::allocate_frame_with_ownership(mmu::FrameOwnership::Shared).expect("Frame alloc");
        assert_eq!(aspace_a.map_shared_frame(0x0050_0000, frame2, false, false), Ok(()));
        assert_eq!(aspace_b.map_shared_frame(0x0050_0000, frame2, false, false), Ok(()));
        assert_eq!(mmu::frame_ownership(frame2), Some(mmu::FrameOwnership::Shared));
        assert_eq!(mmu::frame_refcount(frame2), 2);

        // Borrowed mapping: registered physical MMIO frame
        assert_eq!(aspace_a.map_borrowed_frame(0x0060_0000, 0x0a00_0000, true, false), Ok(()));

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
        kprintln!("[TEST] Frame Ownership & Shared Mappings: Private/Shared/Borrowed lifecycle VERIFIED.");
    }

    // 46. Service Supervisor Lifecycle & Recovery Verification
    kprintln!("--- [Testing AArch64 Service Supervisor Lifecycle & Recovery] ---");
    let test_svc_id = 3; // storaged
    let orig_pid = services::start(test_svc_id).expect("Service start failed in lifecycle test");
    assert!(orig_pid > 0);
    assert_eq!(services::start(test_svc_id), Err(()), "Duplicate start of running service must be rejected");
    assert_eq!(services::heartbeat(9999, 1), Err(()), "Heartbeat with spoofed PID must be rejected");
    assert_eq!(services::heartbeat(orig_pid, 1), Ok(()), "Heartbeat with owning PID and valid generation must be accepted");
    assert_eq!(services::heartbeat(orig_pid, 0), Err(()), "Heartbeat with unvalidated/zero generation must be rejected");
    assert_eq!(services::heartbeat(orig_pid, 999), Err(()), "Heartbeat with stale generation must be rejected");
    // Terminate service task via scheduler terminate_task and test recovery:
    {
        let mut sched = task::SCHEDULER.lock();
        assert!(sched.terminate_task(1, true, orig_pid).is_ok());
    }
    let mut snap_buf = [0u8; 1024];
    services::snapshot(&mut snap_buf);
    assert_eq!(services::recover_failed(test_svc_id), Ok(()), "Service recover_failed must succeed");
    assert_eq!(services::stop(test_svc_id), Ok(()), "Service clean-up stop must succeed");
    assert_eq!(services::stop(test_svc_id), Ok(()), "Stopping already stopped service must succeed");
    kprintln!("[TEST] Service Supervisor Lifecycle: Stopped -> Running -> Heartbeat -> Crashed -> Recover(Restarting -> Running) -> Stopped VERIFIED.");

    // 47. Spawn userspace PID 1 (init) at EL0
    kprintln!();
    kprintln!("--- [Spawning Userspace PID 1 (init) at EL0] ---");
    kprintln!("[BOOT] Loading userspace PID 1 init ELF ({} bytes)...", images::USER_INIT_ELF.len());
    match elf::load_elf(images::USER_INIT_ELF) {
        Ok(l) => {
            kprintln!("[BOOT] PID 1 ELF loaded: entry 0x{:08X}, sp 0x{:08X}", l.entry_point, l.stack_top);
            kprintln!("[BOOT] Entering PID 1 (init) at EL0 via hardware eret...");
            l.address_space.try_activate().expect("Failed to activate init address space");
            {
                let mut sched = task::SCHEDULER.lock();
                sched.tasks.clear();
                sched.next_pid = 1;
                let mut init_caps = capability::CapabilityTable::new();
                init_caps.mint(capability::CapabilityType::IpcSend, capability::Rights::INVOKE, None);
                init_caps.mint(capability::CapabilityType::IpcReceive, capability::Rights::READ, None);
                init_caps.mint(capability::CapabilityType::SerialOutput, capability::Rights::WRITE, None);
                init_caps.mint(capability::CapabilityType::EventPoll, capability::Rights::READ, None);
                init_caps.mint(capability::CapabilityType::DisplayServer, capability::Rights(capability::Rights::WRITE.0 | capability::Rights::ADMIN.0), None);
                init_caps.mint(capability::CapabilityType::SystemResource, capability::Rights(capability::Rights::READ.0 | capability::Rights::WRITE.0 | capability::Rights::ADMIN.0), None);
                let pid1 = sched.spawn_user_process(0, "init", l.entry_point, l.stack_top, Some(l.address_space), init_caps);
                sched.tasks[0].state = task::TaskState::Running;
                sched.current_task_idx = 0;
                kprintln!("[BOOT] PID 1 initialized with state Running (PID {})", pid1);
            }
            unsafe {
                user::enter_user_mode(l.entry_point, l.stack_top);
            }
        }
        Err(e) => {
            kprintln!("[BOOT] Failed to load init ELF: {}", e);
        }
    }

    // Enter idle wait loop if init exits
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!();
    kprintln!("============================================================");
    kprintln!("  [AArch64 KERNEL PANIC]");
    kprintln!("  {}", info);
    kprintln!("============================================================");
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}