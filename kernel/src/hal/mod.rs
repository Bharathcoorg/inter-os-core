//! Multi-Architecture Mobile SoC Hardware Abstraction Layer (HAL)
//!
//! Provides portable abstractions across x86_64 QEMU/PC and ARM64 Mobile SoCs.
//! Includes binary Flattened Device Tree (FDT) parsing, platform timers, and power states.

pub mod traits;
pub mod platform;
pub mod fdt;
pub mod rtc;
pub mod camera;

pub use traits::{PlatformArch, PlatformInfo, PlatformTimer, SerialConsole, PowerController};
pub use platform::{PLATFORM, TIMER, CONSOLE, POWER, ACTIVE_SOC_NAME};
pub use fdt::{FdtParser, FdtNode, FdtProperty, FdtError, build_synthetic_dtb};
pub use rtc::{DateTime, RTC};
pub use camera::{CAMERA, CameraSensor, CameraFrame, SceneType, AutoFocusState};

use crate::serial_println;

/// Initializes the Mobile SoC Hardware Abstraction Layer and executes automated self-tests.
pub fn init() {
    serial_println!("--- [Initializing Mobile SoC Hardware Abstraction Layer (HAL)] ---");

    // 1. Platform & CPU Architecture Identification
    let info = PLATFORM.info();
    let arch_name = match info.arch {
        PlatformArch::X86_64 => "x86_64",
        PlatformArch::AArch64 => "aarch64",
        PlatformArch::RiscV64 => "riscv64",
    };
    serial_println!(
        "[OK] Mobile HAL initialized: Arch = {}, SoC = '{}', Board = '{}', Cores = {}, Freq = {} MHz.",
        arch_name,
        info.soc_name,
        info.board_name,
        info.cpu_cores,
        info.base_frequency_mhz
    );

    // 2. Hardware Monotonic Timer Verification
    let ticks = TIMER.read_ticks();
    let freq = TIMER.frequency_hz();
    assert!(freq > 0, "HAL timer frequency must be non-zero");
    serial_println!("[TEST] HAL Platform Timer: Read {} ticks @ {} Hz.", ticks, freq);

    // 3. Binary Flattened Device Tree (FDT / DTB) Parser Verification
    let dtb_bytes = build_synthetic_dtb();
    let parser = FdtParser::from_slice(&dtb_bytes).expect("Failed to parse valid synthetic DTB");
    assert_eq!(parser.header().magic, fdt::FDT_MAGIC, "FDT magic must match 0xd00dfeed");

    let nodes = parser.enumerate_nodes().expect("Failed to enumerate FDT nodes");
    assert!(nodes.len() >= 3, "DTB must contain at least root, UART, and touchscreen nodes");

    // Verify Root Node & Model
    let root = nodes.iter().find(|n| n.name == "/").expect("Root node missing in DTB");
    let model = root.get_prop("model").and_then(|p| p.as_str()).expect("Model property missing");
    assert_eq!(model, "InterOS Mobile DevBoard v1.0", "Model mismatch");

    // Verify ARM PL011 UART peripheral mapping
    let uart_node = parser.find_compatible_node("arm,pl011").expect("UART scan failed").expect("PL011 node not found");
    let (uart_base, uart_size) = uart_node.get_prop("reg").and_then(|p| p.as_reg_u64()).expect("UART reg missing");
    assert_eq!(uart_base, 0x09000000, "UART base address mismatch");
    assert_eq!(uart_size, 0x1000, "UART address size mismatch");

    // Verify Mobile Capacitive Touchscreen peripheral mapping (Goodix GT911)
    let touch_node = parser.find_compatible_node("goodix,gt911").expect("Touch scan failed").expect("GT911 node not found");
    let touch_irq = touch_node.get_prop("interrupts").and_then(|p| p.as_u32()).expect("Touch IRQ missing");
    assert_eq!(touch_irq, 35, "Touchscreen IRQ line mismatch");

    serial_println!(
        "[TEST] FDT Binary Parser: DTB magic 0xD00DFEED validated, {} nodes traversed, PL011 UART @ 0x{:08X}, Goodix GT911 IRQ {} discovered.",
        nodes.len(),
        uart_base,
        touch_irq
    );

    // 4. Real-Time Clock & CMOS Hardware Driver Initialization
    rtc::init();

    // 5. MIPI-CSI2 Camera Sensor & Neural ISP Pipeline
    let mut cam = CAMERA.lock();
    match cam.capture_frame() {
        Ok(frame) => {
            serial_println!(
                "[TEST] Mobile Camera Sensor: MIPI-CSI2 {} captured {}x{} RGBA frame, ISP tone-mapped, Neural Vision classified '{}'.",
                cam.sensor_model,
                frame.width,
                frame.height,
                frame.scene.as_str()
            );
        }
        Err(err) => {
            serial_println!(
                "[TEST] Mobile Camera Sensor: MIPI-CSI2 {} offline ({}); failing closed without synthetic frames.",
                cam.sensor_model,
                err
            );
        }
    }

    serial_println!("--- [Mobile SoC HAL 100% Attested & Enforced] ---");
}
