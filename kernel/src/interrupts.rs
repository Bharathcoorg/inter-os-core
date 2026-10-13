//! Interrupt Descriptor Table (IDT) & Hardware PIC Controllers
//!
//! Handles CPU hardware exceptions (Breakpoint, Double Fault, Page Fault, GPF)
//! and 8259 PIC external hardware interrupts (PIT system timer, PS/2 keyboard).

use conquer_once::spin::Lazy;
use core::sync::atomic::{AtomicU64, Ordering};
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, serial_println};

/// Master PIC remapped interrupt offset (IRQ 0-7 -> INT 32-39).
pub const PIC_1_OFFSET: u8 = 32;
/// Slave PIC remapped interrupt offset (IRQ 8-15 -> INT 40-47).
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// PIT input clock and requested system timer frequency.
/// The resulting divisor is rounded to the nearest integer, giving a timer
/// rate within normal PIT quantization of 1 kHz (approximately 1 ms/tick).
pub const PIT_BASE_HZ: u32 = 1_193_182;
pub const SYSTEM_TICK_HZ: u32 = 1_000;
pub const PIT_DIVISOR: u16 = (PIT_BASE_HZ / SYSTEM_TICK_HZ) as u16;

/// Global atomic system tick counter (incremented every PIT timer interrupt).
pub static SYSTEM_TICKS: AtomicU64 = AtomicU64::new(0);

/// Dual 8259 Programmable Interrupt Controllers.
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// PS/2 Keyboard decoder state.
static KEYBOARD: Mutex<Option<Keyboard<layouts::Us104Key, ScancodeSet1>>> = Mutex::new(None);

/// Hardware IRQ interrupt vector indices.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Line terminator for every fault-report line.
///
/// This must stay a real carriage return + newline pair. It used to be written
/// with escaped backslashes, which in a Rust byte string is a literal
/// backslash followed by `r` followed by `n` and not a line break at all. Every
/// fault banner therefore printed as one unbroken line, which is what made three
/// separate investigations read a fault address off an unreadable banner and
/// chase the wrong function.
pub const FAULT_LINE_END: &[u8] = b"\r\n";

/// Banner header emitted before a double fault report.
pub const DOUBLE_FAULT_BANNER: &[u8] = b"\r\n[DOUBLE FAULT]\r\n";
/// Label for the stack pointer of a fault that changed privilege level.
///
/// Only in that case does the CPU write an RSP into the interrupt frame, so
/// this label is only ever paired with a frame-derived value.
pub const FAULT_FRAME_STACK_LABEL: &[u8] = b"RSP=";
/// Label for the stack pointer of a same-privilege fault.
///
/// See [`handler_stack_pointer`] for why the frame's own RSP slot cannot be
/// used in this case.
pub const FAULT_HANDLER_STACK_LABEL: &[u8] = b"KSP=";
/// Banner header emitted before a ring-3 page fault report.
pub const USER_PAGE_FAULT_BANNER: &[u8] = b"\r\n[USER PAGE FAULT]\r\n";
/// Banner header emitted before a supervisor page fault report.
pub const KERNEL_PAGE_FAULT_BANNER: &[u8] = b"\r\n[KERNEL PAGE FAULT]\r\n";
/// Banner header emitted before a ring-3 general protection fault report.
pub const USER_GPF_BANNER: &[u8] = b"\r\n[USER GENERAL PROTECTION FAULT]\r\n";
/// Banner header emitted before a supervisor general protection fault report.
pub const KERNEL_GPF_BANNER: &[u8] = b"\r\n[KERNEL GENERAL PROTECTION FAULT]\r\n";
/// Trailing line of a fatal, non-returning fault report.
pub const FAULT_HALT_LINE: &[u8] = b"HALT\r\n";
/// Trailing line of a user fault that terminates the faulting task.
pub const FAULT_TASK_TERMINATED_LINE: &[u8] = b"Terminating user task.\r\n";

#[cfg(not(target_os = "windows"))]
static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();

    // CPU Exceptions
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);

    // Hardware IRQs
    idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
    idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
    idt[34].set_handler_fn(irq2_handler);
    idt[35].set_handler_fn(irq3_handler);
    idt[36].set_handler_fn(irq4_com1_handler);
    idt[37].set_handler_fn(irq5_handler);
    idt[38].set_handler_fn(irq6_handler);
    idt[39].set_handler_fn(irq7_spurious_handler);
    idt[40].set_handler_fn(irq8_rtc_handler);
    idt[41].set_handler_fn(irq9_handler);
    idt[42].set_handler_fn(irq10_handler);
    idt[43].set_handler_fn(irq11_handler);
    idt[44].set_handler_fn(irq12_touch_digitizer_handler);
    idt[45].set_handler_fn(irq13_coproc_handler);
    idt[46].set_handler_fn(irq14_ata_primary_handler);
    idt[47].set_handler_fn(irq15_ata_secondary_handler);

    idt
});

/// Initializes the Interrupt Descriptor Table and loads it into the CPU.
#[cfg(not(target_os = "windows"))]
pub fn init_idt() {
    IDT.load();

    // Initialize PS/2 keyboard decoder
    *KEYBOARD.lock() = Some(Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    ));
}

#[cfg(target_os = "windows")]
pub fn init_idt() {}

/// Initializes and unmasks the 8259 PIC controllers.
#[cfg(not(target_os = "windows"))]
pub fn init_pit() {
    let divisor = PIT_DIVISOR;
    unsafe {
        let mut command = Port::new(0x43);
        let mut channel0 = Port::new(0x40);
        // Channel 0, lobyte/hibyte access, mode 3 square wave, binary mode.
        command.write(0x36u8);
        channel0.write((divisor & 0xff) as u8);
        channel0.write((divisor >> 8) as u8);
    }
}

#[cfg(target_os = "windows")]
pub fn init_pit() {}

#[cfg(not(target_os = "windows"))]
pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();
    }
}

#[cfg(target_os = "windows")]
pub fn init_pics() {}

/// Returns the current total system timer ticks since kernel boot.
#[allow(dead_code)]
pub fn get_system_ticks() -> u64 {
    SYSTEM_TICKS.load(Ordering::Relaxed)
}

// -----------------------------------------------------------------------------
// CPU Exception Handlers
// -----------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[CPU EXCEPTION: BREAKPOINT]");
    serial_println!("{:#?}", stack_frame);
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    // A double fault executes on the dedicated IST stack, but the ordinary
    // formatter is still unsafe: it can lock, allocate, or fault again while
    // the machine is already in a fatal exception state.
    let rip = stack_frame.instruction_pointer.as_u64();
    // Read the frame's RSP only when the CPU actually wrote it.
    let frame_stack_pointer = if frame_has_stack_pointer(&stack_frame) {
        Some(stack_frame.stack_pointer.as_u64())
    } else {
        None
    };
    if !fault_report_bytes(DOUBLE_FAULT_BANNER)
        || !fault_report_line(b"RIP=", rip)
        || !fault_report_stack_pointer(frame_stack_pointer)
        || !fault_report_line(b"ERR=", error_code)
        || !fault_report_bytes(FAULT_HALT_LINE)
    {
        halt_after_fault_report_failure();
    }
    halt_after_fault_report_failure()
}

/// Allocation-free, bounded COM1 writer for exception diagnostics.
///
/// The normal serial formatter takes locks and walks arbitrary formatting
/// arguments. That is unsafe once paging state is suspect because the reporter
/// itself can become the second fault. This path only touches the 16550 PIO
/// registers and uses bounded polling.
#[inline(never)]
fn fault_report_byte(byte: u8) -> bool {
    let mut lsr = Port::<u8>::new(0x3FD);
    let mut thr = Port::<u8>::new(0x3F8);

    for _ in 0..10_000 {
        let status: u8 = unsafe { lsr.read() };
        if status & 0x20 != 0 {
            unsafe { thr.write(byte); }
            return true;
        }
        core::hint::spin_loop();
    }

    false
}

#[inline(never)]
fn fault_report_bytes(bytes: &[u8]) -> bool {
    const MAX_BYTES: usize = 256;
    let len = core::cmp::min(bytes.len(), MAX_BYTES);
    for &byte in &bytes[..len] {
        if !fault_report_byte(byte) {
            return false;
        }
    }
    true
}

#[inline(never)]
fn fault_report_hex(value: u64) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if !fault_report_bytes(b"0x") {
        return false;
    }
    for shift in (0..64).step_by(4).rev() {
        if !fault_report_byte(HEX[((value >> shift) & 0xf) as usize]) {
            return false;
        }
    }
    true
}

#[inline(never)]
fn fault_report_line(label: &[u8], value: u64) -> bool {
    fault_report_bytes(label)
        && fault_report_hex(value)
        && fault_report_bytes(FAULT_LINE_END)
}

/// Reads the stack pointer the handler is actually running on.
///
/// Inline asm rather than a typed accessor for the same reason CR2 is read
/// this way: a fault handler cannot unwrap, panic or format.
#[inline(always)]
fn handler_stack_pointer() -> u64 {
    let stack_pointer: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) stack_pointer,
                         options(nomem, nostack, preserves_flags));
    }
    stack_pointer
}

/// True when the CPU pushed an RSP/SS pair into the interrupt frame.
///
/// The CPU pushes SS and RSP only on a privilege-level change. A ring-0
/// exception runs on the stack that was already current and pushes exactly
/// RIP, CS and RFLAGS, so frame offset 24 is never written by the CPU.
#[inline(always)]
fn frame_has_stack_pointer(stack_frame: &InterruptStackFrame) -> bool {
    stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3
}

/// Reports the stack pointer belonging to the faulting context.
///
/// Pass `Some` only when [`frame_has_stack_pointer`] confirms the CPU wrote the
/// frame's RSP/SS slots, i.e. on a privilege-changing fault. Pass `None` for a
/// same-privilege fault, where the slot was never written.
///
/// `InterruptStackFrame::stack_pointer` is the RSP/SS slot at frame offset 24.
/// Reading it in a same-privilege (ring 0) fault reads memory the CPU never
/// wrote: whatever the kernel stack happened to hold there. That yields a
/// plausible-looking but fabricated address, and a fault report naming the
/// wrong address is worse than one that names none - it is what sent past
/// investigations into the wrong function. For a privilege-changing fault the
/// slot is real and is reported as `RSP`; otherwise the handler's own stack
/// pointer is reported under the distinct label `KSP` so the two can never be
/// confused for the same measurement.
#[inline(never)]
fn fault_report_stack_pointer(frame_stack_pointer: Option<u64>) -> bool {
    match frame_stack_pointer {
        Some(value) => fault_report_line(FAULT_FRAME_STACK_LABEL, value),
        None => fault_report_line(FAULT_HANDLER_STACK_LABEL, handler_stack_pointer()),
    }
}

#[inline(never)]
fn halt_after_fault_report_failure() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // Capture the minimum state before touching any subsystem that may lock,
    // allocate, format, or dereference arbitrary memory.
    //
    // Read the raw CR2 register with inline asm rather than the typed accessor:
    // `Cr2::read()` returns a Result that would have to be unwrapped, and a
    // fault handler cannot unwind, panic or format. This is exactly the
    // hardware value and cannot fail.
    let accessed_address: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) accessed_address,
                         options(nomem, nostack, preserves_flags));
    }
    let instruction_pointer = stack_frame.instruction_pointer.as_u64();
    // The frame's RSP/SS slots exist only when the exception changed privilege
    // level. Reading them unconditionally on a ring-0 fault would report stale
    // kernel stack contents as if they were the faulting stack pointer.
    let frame_stack_pointer = if frame_has_stack_pointer(&stack_frame) {
        Some(stack_frame.stack_pointer.as_u64())
    } else {
        None
    };
    let error_flags = error_code.bits() as u64;
    let is_user = error_code.contains(PageFaultErrorCode::USER_MODE)
        || stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;

    if is_user {
        if !fault_report_bytes(USER_PAGE_FAULT_BANNER)
            || !fault_report_line(b"CR2=", accessed_address)
            || !fault_report_line(b"RIP=", instruction_pointer)
            || !fault_report_stack_pointer(frame_stack_pointer)
            || !fault_report_line(b"ERR=", error_flags)
            || !fault_report_bytes(FAULT_TASK_TERMINATED_LINE)
        {
            halt_after_fault_report_failure();
        }

        // Preserve the existing security boundary: a ring-3 page fault
        // terminates the faulting user task rather than panicking the kernel.
        crate::task::terminate_current();

        // A task terminator is not expected to return. If it ever does, never
        // fall through into the kernel-fault path: that would report a user
        // fault as a kernel fault and lose the distinction this handler exists
        // to preserve.
        #[allow(unreachable_code)]
        halt_after_fault_report_failure();
    }

    if !fault_report_bytes(KERNEL_PAGE_FAULT_BANNER)
        || !fault_report_line(b"CR2=", accessed_address)
        || !fault_report_line(b"RIP=", instruction_pointer)
        || !fault_report_stack_pointer(frame_stack_pointer)
        || !fault_report_line(b"ERR=", error_flags)
        || !fault_report_bytes(FAULT_HALT_LINE)
    {
        halt_after_fault_report_failure();
    }

    // Kernel faults must remain fatal. Do not recurse through panic formatting.
    halt_after_fault_report_failure()
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let rip = stack_frame.instruction_pointer.as_u64();
    let is_user = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    // Read the frame's RSP only when the CPU actually wrote it.
    let frame_stack_pointer = if frame_has_stack_pointer(&stack_frame) {
        Some(stack_frame.stack_pointer.as_u64())
    } else {
        None
    };

    if is_user {
        if !fault_report_bytes(USER_GPF_BANNER)
            || !fault_report_line(b"RIP=", rip)
            || !fault_report_stack_pointer(frame_stack_pointer)
            || !fault_report_line(b"ERR=", error_code)
            || !fault_report_bytes(FAULT_TASK_TERMINATED_LINE)
        {
            halt_after_fault_report_failure();
        }
        crate::task::terminate_current();

        #[allow(unreachable_code)]
        halt_after_fault_report_failure();
    }

    if !fault_report_bytes(KERNEL_GPF_BANNER)
        || !fault_report_line(b"RIP=", rip)
        || !fault_report_stack_pointer(frame_stack_pointer)
        || !fault_report_line(b"ERR=", error_code)
        || !fault_report_bytes(FAULT_HALT_LINE)
    {
        halt_after_fault_report_failure();
    }
    halt_after_fault_report_failure()
}

// -----------------------------------------------------------------------------
// Hardware Interrupt Handlers
// -----------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let ticks = SYSTEM_TICKS.fetch_add(1, Ordering::Relaxed);
    // Periodically log heartbeat every 1000 ticks to avoid serial flood
    if ticks > 0 && ticks % 1000 == 0 {
        serial_println!("[KERNEL TICK HEARTBEAT] System uptime: {} ticks", ticks);
    }
    if ticks > 0 && ticks % 2000 == 0 {
        if let Some(sched) = crate::task::SCHEDULER.try_lock() {
            sched.dump_process_table();
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    crate::serial::poll_serial_rx();

    // Dispatch timer deadlines & wakeups to Event Reactor
    crate::event::on_timer_tick(ticks + 1);

    // Preemptive multitasking time slice accounting & rescheduling
    crate::task::on_timer_tick();
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    if let Some(ref mut keyboard) = *KEYBOARD.lock() {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    DecodedKey::Unicode(character) => {
                        crate::serial_print!("{}", character);
                        crate::input::post_char_event(character);
                    }
                    DecodedKey::RawKey(key) => {
                        serial_println!("[KEY: {:?}]", key);
                    }
                }
            }
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn irq4_com1_handler(_stack_frame: InterruptStackFrame) {
    crate::serial::poll_serial_rx();
    unsafe {
        PICS.lock().notify_end_of_interrupt(36);
    }
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn irq12_touch_digitizer_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::ps2_mouse::handle_irq_byte();
    unsafe {
        PICS.lock().notify_end_of_interrupt(44);
    }
}

#[cfg(not(target_os = "windows"))]
macro_rules! define_pic_eoi_handler {
    ($fn_name:ident, $irq:expr) => {
        extern "x86-interrupt" fn $fn_name(_stack_frame: InterruptStackFrame) {
            unsafe {
                PICS.lock().notify_end_of_interrupt($irq);
            }
        }
    };
}

#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq2_handler, 34);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq3_handler, 35);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq5_handler, 37);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq6_handler, 38);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq7_spurious_handler, 39);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq8_rtc_handler, 40);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq9_handler, 41);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq10_handler, 42);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq11_handler, 43);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq13_coproc_handler, 45);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq14_ata_primary_handler, 46);
#[cfg(not(target_os = "windows"))]
define_pic_eoi_handler!(irq15_ata_secondary_handler, 47);

// -----------------------------------------------------------------------------
// Fault banner formatting tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod fault_banner_tests {
    use super::*;

    /// Every byte string an exception handler can hand to `fault_report_bytes`.
    ///
    /// A banner is only legible if these contain real control characters, so the
    /// list is asserted as a whole rather than one string at a time.
    const EMITTED_LITERALS: &[(&str, &[u8])] = &[
        ("FAULT_LINE_END", FAULT_LINE_END),
        ("DOUBLE_FAULT_BANNER", DOUBLE_FAULT_BANNER),
        ("USER_PAGE_FAULT_BANNER", USER_PAGE_FAULT_BANNER),
        ("KERNEL_PAGE_FAULT_BANNER", KERNEL_PAGE_FAULT_BANNER),
        ("USER_GPF_BANNER", USER_GPF_BANNER),
        ("KERNEL_GPF_BANNER", KERNEL_GPF_BANNER),
        ("FAULT_HALT_LINE", FAULT_HALT_LINE),
        ("FAULT_TASK_TERMINATED_LINE", FAULT_TASK_TERMINATED_LINE),
    ];

    /// Field labels passed to `fault_report_line`.
    const EMITTED_LABELS: &[&[u8]] = &[b"RIP=", b"ERR=", b"CR2="];

    /// Host-side replica of `fault_report_hex`, so the composed report can be
    /// asserted without touching the 16550 ports.
    fn render_hex(value: u64) -> alloc::vec::Vec<u8> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = alloc::vec::Vec::new();
        out.extend_from_slice(b"0x");
        for shift in (0..64).step_by(4).rev() {
            out.push(HEX[((value >> shift) & 0xf) as usize]);
        }
        out
    }

    /// Composes one report exactly as the handlers do: banner, then a labelled
    /// hex field terminated by a real CRLF, then the trailing line.
    fn compose_report(banner: &[u8], label: &[u8], value: u64, trailer: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::new();
        out.extend_from_slice(banner);
        out.extend_from_slice(label);
        out.extend_from_slice(&render_hex(value));
        out.extend_from_slice(FAULT_LINE_END);
        out.extend_from_slice(trailer);
        out
    }

    #[test]
    fn no_emitted_literal_contains_a_literal_backslash() {
        for (name, bytes) in EMITTED_LITERALS {
            assert!(
                !bytes.contains(&b'\\'),
                "{name} contains a literal backslash byte: {bytes:?}"
            );
        }
        for label in EMITTED_LABELS {
            assert!(
                !label.contains(&b'\\'),
                "label {label:?} contains a literal backslash byte"
            );
        }
    }

    #[test]
    fn every_emitted_literal_contains_a_real_crlf() {
        for (name, bytes) in EMITTED_LITERALS {
            assert!(
                bytes.windows(2).any(|pair| pair == b"\r\n"),
                "{name} has no real carriage return / newline pair: {bytes:?}"
            );
            assert!(
                bytes.ends_with(b"\r\n"),
                "{name} does not end with a carriage return / newline pair: {bytes:?}"
            );
        }
    }

    #[test]
    fn every_newline_in_a_banner_is_preceded_by_a_carriage_return() {
        for (name, bytes) in EMITTED_LITERALS {
            for (index, &byte) in bytes.iter().enumerate() {
                if byte == b'\n' {
                    assert!(
                        index > 0 && bytes[index - 1] == b'\r',
                        "{name} has a bare newline at index {index}: {bytes:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_fault_report_spreads_across_distinct_lines() {
        let report = compose_report(
            KERNEL_PAGE_FAULT_BANNER,
            b"CR2=",
            0xffff_ffff_ffff_ffff,
            FAULT_HALT_LINE,
        );

        assert_eq!(
            report,
            b"\r\n[KERNEL PAGE FAULT]\r\nCR2=0xffffffffffffffff\r\nHALT\r\n"
        );
        // A leading CRLF separates the banner from whatever the boot log printed
        // before it, so the report is four lines: blank, banner, field, trailer.
        assert_eq!(
            report.iter().filter(|&&b| b == b'\n').count(),
            4,
            "separator, banner, field and trailer must each occupy their own line"
        );
        assert!(!report.contains(&b'\\'));
    }

    #[test]
    fn double_fault_and_gpf_reports_are_also_multi_line() {
        for (name, banner, trailer) in [
            ("double fault", DOUBLE_FAULT_BANNER, FAULT_HALT_LINE),
            ("user gpf", USER_GPF_BANNER, FAULT_TASK_TERMINATED_LINE),
            ("kernel gpf", KERNEL_GPF_BANNER, FAULT_HALT_LINE),
            ("user page fault", USER_PAGE_FAULT_BANNER, FAULT_TASK_TERMINATED_LINE),
        ] {
            let report = compose_report(banner, b"RIP=", 0x0000_0001_0001_4440, trailer);
            assert_eq!(
                report.iter().filter(|&&b| b == b'\n').count(),
                4,
                "{name} report must render a separator, banner, field and trailer line"
            );
            assert!(!report.contains(&b'\\'), "{name} report contains a backslash");
            assert!(
                report.starts_with(b"\r\n["),
                "{name} report must open with a newline and a bracketed banner"
            );
        }
    }

    /// Guards the whole handler half of the file, not just the constants:
    /// reintroducing an escaped backslash anywhere in a byte string here
    /// silently breaks the banner again, and no compile-time check can see it.
    #[test]
    fn this_file_has_no_escaped_backslash_in_any_byte_string() {
        // Assembled from parts so this test's own source cannot match itself.
        const PAIR: [u8; 2] = [b'\\', b'\\'];
        let source = include_str!("interrupts.rs");

        // Scan up to the test module only; the assertions below have to spell
        // a backslash byte themselves.
        let production = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(head, _tail)| head);

        for (number, line) in production.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            let bad = line
                .as_bytes()
                .windows(PAIR.len())
                .any(|window| window == PAIR);
            assert!(
                !bad,
                "interrupts.rs:{} reintroduces an escaped backslash: {}",
                number + 1,
                trimmed
            );
        }
    }

    // -------------------------------------------------------------------------
    // The interrupt frame's RSP/SS slots
    // -------------------------------------------------------------------------

    /// Byte offsets inside `InterruptStackFrameValue`, which is `#[repr(C)]`.
    const FRAME_RIP: usize = 0;
    const FRAME_CS: usize = 8;
    const FRAME_RFLAGS: usize = 16;
    const FRAME_RSP: usize = 24;

    /// Mirrors the x86_64 crate's `InterruptStackFrameValue` layout.
    ///
    /// Duplicated deliberately: if the crate ever reordered the frame, the
    /// reasoning these tests encode would silently stop applying.
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct FrameLayout {
        instruction_pointer: u64,
        code_segment: u16,
        _reserved1: [u8; 6],
        cpu_flags: u64,
        stack_pointer: u64,
        stack_segment: u16,
        _reserved2: [u8; 6],
    }

    #[test]
    fn the_rsp_slot_sits_after_the_slots_a_same_cpl_fault_always_writes() {
        let frame = FrameLayout {
            instruction_pointer: 0,
            code_segment: 0,
            _reserved1: [0; 6],
            cpu_flags: 0,
            stack_pointer: 0,
            stack_segment: 0,
            _reserved2: [0; 6],
        };
        let base = &frame as *const FrameLayout as usize;

        assert_eq!(
            &frame.instruction_pointer as *const u64 as usize - base,
            FRAME_RIP
        );
        assert_eq!(&frame.code_segment as *const u16 as usize - base, FRAME_CS);
        assert_eq!(&frame.cpu_flags as *const u64 as usize - base, FRAME_RFLAGS);
        assert_eq!(
            &frame.stack_pointer as *const u64 as usize - base,
            FRAME_RSP
        );
    }

    /// A ring-0 exception pushes exactly RIP, CS and RFLAGS, so the RSP slot at
    /// offset 24 is memory the CPU never wrote for this fault.
    #[test]
    fn a_same_privilege_fault_does_not_establish_the_frame_rsp_slot() {
        const BYTES_PUSHED_BY_CPU_WITHOUT_A_PRIVILEGE_CHANGE: usize = 24;
        assert_eq!(FRAME_RSP, BYTES_PUSHED_BY_CPU_WITHOUT_A_PRIVILEGE_CHANGE);
        assert!(
            FRAME_RSP >= FRAME_RFLAGS + 8,
            "the RSP slot must sit strictly after the last slot the CPU always writes"
        );
    }

    /// The two stack-pointer labels must never be interchangeable, so a reader
    /// can tell a frame-derived RSP from the handler's own stack pointer.
    #[test]
    fn the_two_stack_pointer_labels_are_distinguishable() {
        assert_eq!(FAULT_FRAME_STACK_LABEL, b"RSP=");
        assert_eq!(FAULT_HANDLER_STACK_LABEL, b"KSP=");
        assert_ne!(FAULT_FRAME_STACK_LABEL, FAULT_HANDLER_STACK_LABEL);
        for label in [FAULT_FRAME_STACK_LABEL, FAULT_HANDLER_STACK_LABEL] {
            assert!(
                !label.contains(&b'\\'),
                "stack pointer label must not contain a backslash: {label:?}"
            );
        }
    }

    /// A kernel fault report must label its stack pointer as the handler's own
    /// and never claim a frame RSP the CPU did not write.
    #[test]
    fn a_kernel_fault_report_labels_its_stack_pointer_as_the_handlers_own() {
        let mut report = alloc::vec::Vec::new();
        report.extend_from_slice(KERNEL_PAGE_FAULT_BANNER);
        report.extend_from_slice(FAULT_HANDLER_STACK_LABEL);
        report.extend_from_slice(&render_hex(0x0000_0000_0008_0000));
        report.extend_from_slice(FAULT_LINE_END);

        assert_eq!(
            report,
            b"\r\n[KERNEL PAGE FAULT]\r\nKSP=0x0000000000080000\r\n"
        );
        assert!(
            !report.windows(3).any(|window| window == b"RSP"),
            "a same-privilege report must not claim a frame RSP: {report:?}"
        );
        assert!(!report.contains(&b'\\'));
    }
}

