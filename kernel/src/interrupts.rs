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
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, serial_println};

/// Master PIC remapped interrupt offset (IRQ 0-7 -> INT 32-39).
pub const PIC_1_OFFSET: u8 = 32;
/// Slave PIC remapped interrupt offset (IRQ 8-15 -> INT 40-47).
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

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
pub fn init_idt() {
    IDT.load();

    // Initialize PS/2 keyboard decoder
    *KEYBOARD.lock() = Some(Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    ));
}

/// Initializes and unmasks the 8259 PIC controllers.
pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();
    }
}

/// Returns the current total system timer ticks since kernel boot.
#[allow(dead_code)]
pub fn get_system_ticks() -> u64 {
    SYSTEM_TICKS.load(Ordering::Relaxed)
}

// -----------------------------------------------------------------------------
// CPU Exception Handlers
// -----------------------------------------------------------------------------

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[CPU EXCEPTION: BREAKPOINT]");
    serial_println!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    panic!(
        "[CPU EXCEPTION: DOUBLE FAULT] (code: {})\n{:#?}",
        error_code, stack_frame
    );
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let accessed_address = Cr2::read();
    let is_user = error_code.contains(PageFaultErrorCode::USER_MODE)
        || stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;

    if is_user {
        serial_println!("\n[USER EXCEPTION: PAGE FAULT]");
        serial_println!("Accessed Address: {:?}", accessed_address);
        serial_println!("Error Flags:      {:?}", error_code);
        serial_println!("Faulting RIP:     {:?}", stack_frame.instruction_pointer);
        serial_println!("[SECURITY] Terminating faulted user process to protect microkernel integrity.");
        crate::task::terminate_current();
    }

    serial_println!("\n[CPU EXCEPTION: PAGE FAULT]");
    serial_println!("Accessed Address: {:?}", accessed_address);
    serial_println!("Error Flags:      {:?}", error_code);
    serial_println!("{:#?}", stack_frame);
    panic!("Fatal unhandled page fault in kernel context.");
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    let is_user = stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3;
    if is_user {
        serial_println!("\n[USER EXCEPTION: GENERAL PROTECTION FAULT (code: {})]", error_code);
        serial_println!("Faulting RIP: {:?}", stack_frame.instruction_pointer);
        serial_println!("[SECURITY] Terminating faulted user process to protect microkernel integrity.");
        crate::task::terminate_current();
    }

    panic!(
        "[CPU EXCEPTION: GENERAL PROTECTION FAULT] (code: {})\n{:#?}",
        error_code, stack_frame
    );
}

// -----------------------------------------------------------------------------
// Hardware Interrupt Handlers
// -----------------------------------------------------------------------------

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let ticks = SYSTEM_TICKS.fetch_add(1, Ordering::Relaxed);
    // Periodically log heartbeat every 1000 ticks to avoid serial flood
    if ticks > 0 && ticks % 1000 == 0 {
        serial_println!("[KERNEL TICK HEARTBEAT] System uptime: {} ticks", ticks);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    // Dispatch timer deadlines & wakeups to Event Reactor
    crate::event::on_timer_tick(ticks + 1);

    // Preemptive multitasking time slice accounting & rescheduling
    crate::task::on_timer_tick();
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    if let Some(ref mut keyboard) = *KEYBOARD.lock() {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    DecodedKey::Unicode(character) => {
                        crate::serial_print!("{}", character);
                        crate::framebuffer::on_key_press(character);
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

extern "x86-interrupt" fn irq12_touch_digitizer_handler(_stack_frame: InterruptStackFrame) {
    let mut port = Port::new(0x60);
    let _byte: u8 = unsafe { port.read() };
    unsafe {
        PICS.lock().notify_end_of_interrupt(44);
    }
}

macro_rules! define_pic_eoi_handler {
    ($fn_name:ident, $irq:expr) => {
        extern "x86-interrupt" fn $fn_name(_stack_frame: InterruptStackFrame) {
            unsafe {
                PICS.lock().notify_end_of_interrupt($irq);
            }
        }
    };
}

define_pic_eoi_handler!(irq2_handler, 34);
define_pic_eoi_handler!(irq3_handler, 35);
define_pic_eoi_handler!(irq4_com1_handler, 36);
define_pic_eoi_handler!(irq5_handler, 37);
define_pic_eoi_handler!(irq6_handler, 38);
define_pic_eoi_handler!(irq7_spurious_handler, 39);
define_pic_eoi_handler!(irq8_rtc_handler, 40);
define_pic_eoi_handler!(irq9_handler, 41);
define_pic_eoi_handler!(irq10_handler, 42);
define_pic_eoi_handler!(irq11_handler, 43);
define_pic_eoi_handler!(irq13_coproc_handler, 45);
define_pic_eoi_handler!(irq14_ata_primary_handler, 46);
define_pic_eoi_handler!(irq15_ata_secondary_handler, 47);

