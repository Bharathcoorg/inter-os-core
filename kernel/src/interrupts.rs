// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Interrupt Descriptor Table (IDT) & Hardware PIC Controllers
//!
//! Handles CPU hardware exceptions (Breakpoint, Double Fault, Page Fault, GPF)
//! and 8259 PIC external hardware interrupts (PIT system timer, PS/2 keyboard).

#[cfg(not(target_os = "windows"))]
use conquer_once::spin::Lazy;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(target_os = "windows"))]
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::port::Port;
#[cfg(not(target_os = "windows"))]
use x86_64::structures::idt::{InterruptDescriptorTable, PageFaultErrorCode};
use x86_64::structures::idt::InterruptStackFrame;

#[cfg(not(target_os = "windows"))]
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

/// Global atomic system tick counter (milliseconds of real time since boot).
pub static SYSTEM_TICKS: AtomicU64 = AtomicU64::new(0);

/// The one-second liveness line, printed by the kernel's loop rather than the
/// tick that asks for it.
///
/// One line is small, but a line written from the handler is written with IF
/// clear, so a touch that arrives during its byte-at-a-time transmit waits for
/// the serial port before it can be serviced. The stamp travels with the flag
/// so the number stays the one the second actually crossed at.
pub static TICK_HEARTBEAT_DUE: AtomicBool = AtomicBool::new(false);
pub static TICK_HEARTBEAT_AT: AtomicU64 = AtomicU64::new(0);

/// The last shape of the task table the console was shown, as a
/// [`process_table_signature`].
///
/// `u64::MAX` is the sentinel "the console has not been shown a table yet", so
/// the kernel's loop puts exactly one table up after boot and then says nothing
/// until the table genuinely changes.
///
/// WHY this replaced `PROCESS_TABLE_DUMP_DUE`: that flag was armed by the timer
/// tick every two seconds, so an idle device with sixteen tasks printed
/// `17 lines / 2 s` of `[PROC]` table that nobody had asked for. Silence is the
/// point of this OS's idle state, and the same rule the RTC follows applies
/// here: report on change, not on the clock.
pub static PROCESS_TABLE_SHOWN_SIGNATURE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Milliseconds between two looks at the task table.
///
/// WHY a bound on the *sampling* and not on the printing: whether a table is
/// printed is decided purely by [`process_table_signature`] changing, but
/// working that out walks the task list under the scheduler lock. Once a second
/// is far more often than the table changes shape, and well inside the cost of
/// `services::supervise`, which already takes the same lock on every pass of
/// the kernel loop.
pub const PROCESS_TABLE_SAMPLE_TICKS: u64 = 1_000;

/// The tick at which the task table was last sampled for change.
pub static PROCESS_TABLE_SAMPLED_AT: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Real-time clock source
// ---------------------------------------------------------------------------
//
// WHY the tick counter is not the number of timer interrupts: the PIT's
// delivery is scheduled by the *host's* timer, and a host that resolves timers
// at 15.6 ms — which is what a Windows host gives a VM unless it raises its own
// resolution — services a 1 kHz PIT only ~64 times a second. Every timeout,
// animation frame, event-poll deadline and gesture in this OS is denominated in
// these ticks, so a guest that counted deliveries ran its whole clock as much
// as 20x slower than the device it sits in: a 180 ms volume HUD held for
// 3.6 seconds and a 100 ms `event_poll` slept for 2. The HPET main counter is a
// free-running real-time counter that costs one MMIO read per delivery, so the
// tick counter can advance by the milliseconds that actually passed whatever
// rate the host delivers the timer at.
//
// The counter register's address is fixed by the HPET specification, and the
// page is mapped no-cache rather than through the direct map, because the
// bootloader's direct map covers RAM and this aperture is not RAM.
#[cfg(not(target_os = "windows"))]
const HPET_PHYS_BASE: u64 = 0xFED0_0000;
/// The E1000's aperture owns the first two pages at this virtual base.
#[cfg(not(target_os = "windows"))]
const HPET_VIRT_BASE: u64 = 0xFFFF_9000_0010_0000;
/// Every 64-bit HPET register sits in a 16-byte slot: the low half at the slot
/// address, the high half four bytes above it. Writing the enable bit to 0x08
/// instead - the offset the spec-free guess suggests - aims at a reserved hole,
/// and the device drops it without saying so.
#[cfg(not(target_os = "windows"))]
const HPET_GEN_CAP_ID: usize = 0x00;
#[cfg(not(target_os = "windows"))]
const HPET_GEN_CFG: usize = 0x10;
#[cfg(not(target_os = "windows"))]
const HPET_MAIN_CNT_LO: usize = 0xF0;
#[cfg(not(target_os = "windows"))]
const HPET_MAIN_CNT_HI: usize = 0xF4;
#[cfg(not(target_os = "windows"))]
const HPET_ENABLE: u64 = 1 << 0;
/// Femtoseconds in one millisecond, the unit the counter period is reported in.
const FEMTOSECONDS_PER_MS: u64 = 1_000_000_000_000;
/// A counter slower than 1 MHz cannot be divided into whole milliseconds
/// without rounding away, and an all-ones read (no device behind the aperture)
/// is far above this bound, so the bound doubles as the absent-device test.
const MAX_COUNTER_PERIOD_FS: u64 = 1_000_000_000;
/// The most time one delivery may account for. Beyond a second the vCPU was
/// stopped rather than slow, and the clock loses that interval instead of
/// jumping every deadline in the OS at once. The RTC owns the wall clock.
pub const MAX_TICKS_PER_DELIVERY: u64 = 1_000;

/// `0` unprobed, `1` the HPET counter, `2` timer deliveries only.
#[cfg(not(target_os = "windows"))]
static CLOCK_SOURCE: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_os = "windows"))]
static HPET_COUNTS_PER_MS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_os = "windows"))]
static HPET_LAST_COUNT: AtomicU64 = AtomicU64::new(0);

/// Whole milliseconds spanned by `delta` counter steps, or `None` when the
/// counter cannot be expressed in milliseconds at all.
pub const fn millis_between(delta: u64, counts_per_ms: u64) -> Option<u64> {
    if counts_per_ms < 2 {
        return None;
    }
    Some(delta / counts_per_ms)
}

/// What one timer delivery does to the clock: the ticks to charge and the
/// counter value the next delivery ages from.
///
/// The anchor is deliberately not `now`. Charging the whole milliseconds that
/// passed and then ageing from wherever the counter happened to be discards the
/// fraction of a millisecond left over, and on a host that delivers every
/// 1.23 ms that discard is 0.23 ms per delivery - the clock runs at 813 ticks to
/// the real second and every timeout in the OS is 23 % late, the same class of
/// error as counting deliveries was. Leaving the leftover un-credited keeps it
/// for the next delivery, so the average is exactly the counter's rate.
///
/// `Some(0)` therefore charges nothing and moves nothing rather than rounding
/// up to one tick: a host that delivers faster than the counter resolves would
/// otherwise run the clock faster than real time. It cannot stall it, because an
/// interval that is too short to credit is always still in the anchor for the
/// next delivery to collect.
pub const fn age_clock(now: u64, previous: u64, counts_per_ms: u64) -> (u64, u64) {
    let delta = now.wrapping_sub(previous);
    match millis_between(delta, counts_per_ms) {
        // Only reachable with a counter too coarse to divide into milliseconds,
        // where a delivery is the only thing worth counting.
        None => (1, now),
        Some(0) => (0, previous),
        Some(ms) if ms > MAX_TICKS_PER_DELIVERY => (MAX_TICKS_PER_DELIVERY, now),
        Some(ms) => (ms, previous.wrapping_add(ms.wrapping_mul(counts_per_ms))),
    }
}

/// `counts_per_ms` decoded from one HPET capability register, or `None` when
/// the register does not describe a usable free-running counter.
///
/// The counter clock period is the register's upper 32 bits, in femtoseconds:
/// QEMU composes it as `HPET_CLK_PERIOD * FS_PER_NS << 32`, and the guest this
/// kernel boots on reads `0x00989680_8086a201` — 10,000,000 fs, the 100 MHz
/// counter QEMU emulates. Nothing here trusts that number on its own: the
/// caller has to watch the counter move before the clock is believed.
pub const fn hpet_counts_per_ms(capability: u64) -> Option<u64> {
    let period_fs = capability >> 32;
    if period_fs == 0 || period_fs > MAX_COUNTER_PERIOD_FS {
        return None;
    }
    // Rounded so a counter whose period does not divide a millisecond exactly
    // is not biased down by a whole fraction of a step.
    let counts = (FEMTOSECONDS_PER_MS + period_fs / 2) / period_fs;
    if counts < 2 {
        return None;
    }
    Some(counts)
}

/// One 64-bit main counter reading, stitched from the two 32-bit halves.
///
/// The counter is addressed as a low half and a high half four bytes apart, so
/// a value read across a roll of the low half would otherwise come out either
/// one whole 32-bit cycle early or late. Reading low, high, low and keeping the
/// larger low pairs the high half with a low half it can genuinely follow: the
/// low half only rises between the two reads unless it rolled past the high
/// half's boundary, and when it rolled the pre-roll low is the one that belongs
/// to this high.
pub const fn main_count(high: u32, first_low: u32, second_low: u32) -> u64 {
    let low = if second_low >= first_low {
        second_low
    } else {
        first_low
    };
    ((high as u64) << 32) | low as u64
}

/// Whether a counter that moved from `previous` to `current` crossed a
/// multiple of `period`.
///
/// WHY a crossing test rather than `current % period == 0`: the counter now
/// advances in jumps, so a modulo would print only when a jump happened to
/// land exactly on the boundary and would silently skip the rest.
pub const fn period_crossed(previous: u64, current: u64, period: u64) -> bool {
    period > 0 && current / period != previous / period
}

/// Dual 8259 Programmable Interrupt Controllers.
pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// PS/2 Keyboard decoder state.
#[cfg(not(target_os = "windows"))]
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
    // WHY written to by hand: `pic8259` exposes `initialize`,
    // `handle_interrupt` and `notify_end_of_interrupt` and nothing else, and
    // `initialize` deliberately restores whatever masks the chip already held --
    // so no call in the crate can lift one. Firmware leaves COM1's IRQ4 masked,
    // which made the UART's own data-ready interrupt inert: setting IER without
    // clearing this bit changes nothing at all.
    unmask_pic_irq(4);
}

#[cfg(target_os = "windows")]
pub fn init_pics() {}

/// Clears one IRQ line's mask bit in its PIC's interrupt mask register.
///
/// The slave is indexed from its own zero, so IRQ8 becomes bit 0 of port 0xA1.
/// Reading first and writing back keeps every other line exactly as firmware
/// left it: this opens one interrupt, not a reset of the controller.
#[cfg(not(target_os = "windows"))]
fn unmask_pic_irq(irq: u8) {
    let (imr_port, bit) = if irq >= 8 {
        (0xA1u16, irq - 8)
    } else {
        (0x21u16, irq)
    };
    unsafe {
        let mut port = Port::<u8>::new(imr_port);
        let held = port.read();
        port.write(held & !(1u8 << bit));
    }
}

/// Maps and enables the HPET main counter, returning its period in counts per
/// millisecond once it has been seen to advance.
///
/// The counter only runs while the global enable bit is set, and firmware does
/// not always set it, so the bit is raised here and the counter is then read
/// twice: trusting a clock without having observed it move is a claim about a
/// device that may not be there.
///
/// The error is the reason the clock fell back, because the difference between
/// "this platform has no HPET" and "this mapping went wrong" is only visible
/// from inside the guest that tried to read it.
#[cfg(not(target_os = "windows"))]
unsafe fn probe_hpet() -> Result<u64, &'static str> {
    use crate::memory::map_mmio_region;
    use x86_64::{PhysAddr, VirtAddr};

    let mapped = map_mmio_region(
        PhysAddr::new(HPET_PHYS_BASE),
        0x400,
        VirtAddr::new(HPET_VIRT_BASE),
    )
    .map_err(|_| "the HPET page could not be mapped")?;
    let base = mapped.as_u64() as usize;
    let read = |offset: usize| core::ptr::read_volatile((base + offset) as *const u64);
    let write = |offset: usize, value: u64| {
        core::ptr::write_volatile((base + offset) as *mut u64, value)
    };
    let counter = || {
        let first_low = core::ptr::read_volatile((base + HPET_MAIN_CNT_LO) as *const u32);
        let high = core::ptr::read_volatile((base + HPET_MAIN_CNT_HI) as *const u32);
        let second_low = core::ptr::read_volatile((base + HPET_MAIN_CNT_LO) as *const u32);
        main_count(high, first_low, second_low)
    };

    let capability = read(HPET_GEN_CAP_ID);
    let counts_per_ms = hpet_counts_per_ms(capability)
        .ok_or("the HPET capability register does not describe a usable counter period")?;
    let config_before = read(HPET_GEN_CFG);
    write(HPET_GEN_CFG, config_before | HPET_ENABLE);
    let config_after = read(HPET_GEN_CFG);

    // Watch for movement with a budget rather than a fixed pause: the counter
    // steps every 10 ns, so anything that is really running will move well
    // inside this loop, and a device that does not move is reported with the
    // registers it answered.
    let before = counter();
    let mut after = before;
    for _ in 0..200_000 {
        after = counter();
        if after != before {
            break;
        }
        core::hint::spin_loop();
    }
    serial_println!(
        "[HPET] capability 0x{:x}, cfg 0x{:x} -> 0x{:x}, {} steps/ms, counter {} -> {}",
        capability,
        config_before,
        config_after,
        counts_per_ms,
        before,
        after
    );
    if after > before {
        Ok(counts_per_ms)
    } else {
        Err("the HPET main counter did not advance")
    }
}

/// Chooses the clock the tick counter follows. Called once before the timer
/// interrupt is unmasked, and says in plain words which clock the device
/// actually got, because the difference shows up in every timeout the shell
/// draws.
#[cfg(not(target_os = "windows"))]
pub fn init_timekeeping() -> &'static str {
    match unsafe { probe_hpet() } {
        Ok(counts) => {
            HPET_COUNTS_PER_MS.store(counts, Ordering::Relaxed);
            HPET_LAST_COUNT.store(unsafe { hpet_main_count() }, Ordering::Relaxed);
            CLOCK_SOURCE.store(1, Ordering::Relaxed);
            "HPET main counter: 1 tick is 1 ms of real time"
        }
        Err(reason) => {
            CLOCK_SOURCE.store(2, Ordering::Relaxed);
            serial_println!("[TIMEKEEPING] falling back: {}", reason);
            "timer deliveries only: the device clock runs as fast as this host delivers the timer"
        }
    }
}

#[cfg(target_os = "windows")]
pub fn init_timekeeping() -> &'static str {
    "host test build: no hardware clock is read"
}

/// One read of the HPET main counter, through the no-cache page it is mapped to.
#[cfg(not(target_os = "windows"))]
#[inline]
unsafe fn hpet_main_count() -> u64 {
    let base = HPET_VIRT_BASE as usize;
    let first_low = core::ptr::read_volatile((base + HPET_MAIN_CNT_LO) as *const u32);
    let high = core::ptr::read_volatile((base + HPET_MAIN_CNT_HI) as *const u32);
    let second_low = core::ptr::read_volatile((base + HPET_MAIN_CNT_LO) as *const u32);
    main_count(high, first_low, second_low)
}

/// The ticks one timer delivery should age the clock by, from the counter.
#[cfg(not(target_os = "windows"))]
#[inline]
fn elapsed_ticks() -> u64 {
    if CLOCK_SOURCE.load(Ordering::Relaxed) != 1 {
        return 1;
    }
    let now = unsafe { hpet_main_count() };
    let previous = HPET_LAST_COUNT.load(Ordering::Relaxed);
    let (ms, anchor) = age_clock(now, previous, HPET_COUNTS_PER_MS.load(Ordering::Relaxed));
    HPET_LAST_COUNT.store(anchor, Ordering::Relaxed);
    ms
}

/// Returns the number of milliseconds since kernel boot.
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn fault_report_stack_pointer(frame_stack_pointer: Option<u64>) -> bool {
    match frame_stack_pointer {
        Some(value) => fault_report_line(FAULT_FRAME_STACK_LABEL, value),
        None => fault_report_line(FAULT_HANDLER_STACK_LABEL, handler_stack_pointer()),
    }
}

#[inline(never)]
#[cfg_attr(target_os = "windows", allow(dead_code))]
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
    // WHY the clock is read here and not simply incremented: one delivery can
    // stand for many milliseconds of real time, and the whole OS measures
    // timeouts in these ticks. See the clock source at the top of this file.
    let elapsed = elapsed_ticks();
    let previous = SYSTEM_TICKS.fetch_add(elapsed, Ordering::Relaxed);
    let ticks = previous.saturating_add(elapsed);
    if period_crossed(previous, ticks, 1000) {
        TICK_HEARTBEAT_AT.store(ticks, Ordering::Relaxed);
        TICK_HEARTBEAT_DUE.store(true, Ordering::Relaxed);
    }
    // WHY nothing arms the process table here any more: the tick used to set a
    // dump-due flag every 2000 ms, which put a header plus one `[PROC]` line per
    // task on an idle console roughly four times a minute for as long as the
    // device was powered. The kernel's loop now looks at the task table itself
    // and prints only when its shape has changed, or when someone asks for it by
    // name with `PROC_LIST`.

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    crate::serial::poll_serial_rx();

    // Dispatch timer deadlines & wakeups to Event Reactor
    crate::event::on_timer_tick(ticks);

    // Preemptive multitasking time slice accounting & rescheduling
    crate::task::on_timer_tick(elapsed);
}

/// Signature of the task table's *lifecycle* shape.
///
/// WHY these four numbers rather than every field of the `[PROC]` lines: the
/// per-task `state=` column flips `Ready` -> `Blocked` -> `Running` many times a
/// second as tasks take IPC waits, event polls and time slices. That is not a
/// change anyone asked to hear about, and printing on it would put the idle
/// device's chatter straight back. What a reader of the process panel needs
/// told is a process appearing, disappearing, dying, or taking the foreground,
/// which is exactly what these count. Task ids are never reused, so no two
/// different sets of ids can share a size and a sum.
pub fn process_table_signature(sched: &crate::task::scheduler::Scheduler) -> u64 {
    use crate::task::TaskState;

    let mut live: u64 = 0;
    let mut exiting: u64 = 0;
    let mut id_sum: u64 = 0;
    let foreground = sched.active_foreground_task().map(|id| id.0).unwrap_or(u64::MAX);
    for task in sched.tasks_iter() {
        match task.state {
            TaskState::Zombie | TaskState::Terminated => exiting += 1,
            _ => live += 1,
        }
        // Saturating, so a pathological number of tasks folds into the range
        // instead of wrapping and hiding a change behind an equal sum.
        id_sum = id_sum.saturating_add(task.id.0);
    }
    (live << 44) ^ (exiting << 24) ^ id_sum.rotate_left(7) ^ foreground.rotate_right(11)
}

/// Reports whether `sched`'s table differs from the one the console was last
/// shown, and records it as shown.
///
/// WHY this is separate from the printing: the decision is the whole of the
/// "on change, never on a timer" rule, and it has to be testable on the host,
/// where `dump_process_table` cannot run because there is no page table to name
/// in a `[PROC]` line.
pub fn process_table_changed(sched: &crate::task::scheduler::Scheduler) -> bool {
    let signature = process_table_signature(sched);
    if signature == PROCESS_TABLE_SHOWN_SIGNATURE.load(Ordering::Relaxed) {
        return false;
    }
    PROCESS_TABLE_SHOWN_SIGNATURE.store(signature, Ordering::Relaxed);
    true
}

/// Prints the process table when its shape differs from the last one the
/// console was shown, and reports whether it printed.
pub fn dump_process_table_if_changed(sched: &crate::task::scheduler::Scheduler) -> bool {
    if !process_table_changed(sched) {
        return false;
    }
    sched.dump_process_table();
    true
}

/// Prints the process table for an explicit `PROC_LIST` request and returns how
/// many tasks it showed, or `None` if the scheduler was busy.
///
/// WHY the request also updates [`PROCESS_TABLE_SHOWN_SIGNATURE`]: the console
/// has just been shown exactly that table. Leaving the signature stale would
/// make the kernel's loop repeat the whole dump a second later, which is the
/// chatter this change exists to remove.
pub fn process_table_dump_now() -> Option<usize> {
    let sched = crate::task::SCHEDULER.try_lock()?;
    let tasks = sched.task_count();
    PROCESS_TABLE_SHOWN_SIGNATURE.store(process_table_signature(&sched), Ordering::Relaxed);
    sched.dump_process_table();
    Some(tasks)
}

/// Prints what the tick deferred, in a context where its bytes block nothing.
///
/// WHY this is called from the kernel's loop and not from the tick: the dump is
/// one `[PROC]` line per task, and a handler writes them with IF clear, so the
/// whole dump was an interrupt-off window. Content and the Studio panel that
/// consumes it are unchanged -- only the context it runs in moved, and the
/// trigger moved from "two seconds passed" to "the task table changed".
#[cfg(not(target_os = "windows"))]
pub fn service_deferred_diagnostics() {
    if TICK_HEARTBEAT_DUE.swap(false, Ordering::Relaxed) {
        crate::serial_println!(
            "[KERNEL TICK HEARTBEAT] System uptime: {} ticks",
            TICK_HEARTBEAT_AT.load(Ordering::Relaxed)
        );
    }

    let now = SYSTEM_TICKS.load(Ordering::Relaxed);
    let last_sample = PROCESS_TABLE_SAMPLED_AT.load(Ordering::Relaxed);
    if now.saturating_sub(last_sample) < PROCESS_TABLE_SAMPLE_TICKS {
        return;
    }
    PROCESS_TABLE_SAMPLED_AT.store(now, Ordering::Relaxed);
    if let Some(sched) = crate::task::SCHEDULER.try_lock() {
        dump_process_table_if_changed(&sched);
    }
}

#[cfg(target_os = "windows")]
pub fn service_deferred_diagnostics() {}

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
extern "x86-interrupt" fn irq14_ata_primary_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        let mut status_port = x86_64::instructions::port::PortReadOnly::<u8>::new(0x1F7);
        let _ = status_port.read();
        PICS.lock().notify_end_of_interrupt(46);
    }
}

#[cfg(not(target_os = "windows"))]
extern "x86-interrupt" fn irq15_ata_secondary_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        let mut status_port = x86_64::instructions::port::PortReadOnly::<u8>::new(0x177);
        let _ = status_port.read();
        PICS.lock().notify_end_of_interrupt(47);
    }
}

// -----------------------------------------------------------------------------
// Real-time clock tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod timekeeping_tests {
    use super::*;

    /// One HPET capability register: the flags byte QEMU writes plus the
    /// counter period in femtoseconds, which the register carries in its upper
    /// 32 bits.
    const fn capability(period_fs: u64) -> u64 {
        (period_fs << 32) | 0x8086_a201
    }

    #[test]
    fn a_qemu_hpet_counts_a_millisecond_in_hundred_thousand_steps() {
        // QEMU programs 10 ns per step; a real HPET is 69.8 ns (14.318 MHz).
        assert_eq!(hpet_counts_per_ms(capability(10_000_000)), Some(100_000));
        assert_eq!(hpet_counts_per_ms(capability(69_845_857)), Some(14_317));
        // The register this device booted with, read from the live guest.
        assert_eq!(hpet_counts_per_ms(0x0098_9680_8086_a201), Some(100_000));
    }

    #[test]
    fn an_absent_or_absurd_counter_period_is_refused() {
        // What an unmapped aperture reads back: every bit set, so the period is
        // 1.8e19 fs, far outside anything a clock could be.
        assert_eq!(hpet_counts_per_ms(u64::MAX), None);
        // A zero period is what a register that is all flags reads back.
        assert_eq!(hpet_counts_per_ms(0x8086_a201), None);
        // Slower than one step per microsecond cannot be divided into whole
        // milliseconds without rounding the clock away.
        assert_eq!(hpet_counts_per_ms(capability(2_000_000_000)), None);
    }

    #[test]
    fn a_delivery_charges_the_milliseconds_that_actually_passed() {
        // QEMU programs 10 ns per step; a real HPET is 69.8 ns (14.318 MHz).
        assert_eq!(age_clock(15_600_000, 0, 100_000), (156, 15_600_000));
        // 1.23 ms is what this host's deliveries are actually spaced by. One
        // tick is charged and the remaining 0.23 ms stays owed, not discarded.
        assert_eq!(age_clock(123_000, 0, 100_000), (1, 100_000));
        assert_eq!(age_clock(246_000, 100_000, 100_000), (1, 200_000));
    }

    #[test]
    fn ten_deliveries_at_the_measured_spacing_buy_only_the_time_that_passed() {
        // The regression this replaces floored every delivery to one whole
        // millisecond, so 12.3 ms of real time bought 10 ticks and each timeout
        // in the OS ran 23 % long. Charging whole milliseconds and carrying the
        // rest gives the counter's own rate.
        let mut now = 0;
        let mut anchor = 0;
        let mut charged = 0;
        for _ in 0..10 {
            now += 123_000;
            let (ms, next) = age_clock(now, anchor, 100_000);
            charged += ms;
            anchor = next;
        }
        assert_eq!(charged, 12);
        assert_eq!(now - anchor, 30_000, "the part of a millisecond still owed");
    }

    #[test]
    fn a_delivery_faster_than_the_counter_earns_nothing_and_loses_nothing() {
        // A host delivering every 0.4 ms: two of them are 0.8 ms, still no whole
        // millisecond to charge, and the third is the one that pays. Rounding
        // each up here would run the clock faster than the device it measures.
        assert_eq!(age_clock(40_000, 0, 100_000), (0, 0));
        assert_eq!(age_clock(80_000, 0, 100_000), (0, 0));
        assert_eq!(age_clock(120_000, 0, 100_000), (1, 100_000));
    }

    #[test]
    fn a_stopped_vcpu_cannot_jump_every_deadline_at_once() {
        // An hour of stopped counter is charged as one second and the rest is
        // dropped: holding the remainder would spend 3 600 deliveries catching
        // up and expire every timeout, animation and lease in the OS at once.
        let one_hour = 3_600 * 100_000;
        assert_eq!(age_clock(one_hour, 0, 100_000), (MAX_TICKS_PER_DELIVERY, one_hour));
        assert_eq!(age_clock(1_000 * 100_000, 0, 100_000), (1_000, 1_000 * 100_000));
        assert_eq!(age_clock(999 * 100_000, 0, 100_000), (999, 999 * 100_000));
    }

    #[test]
    fn a_clock_without_a_usable_period_counts_deliveries() {
        assert_eq!(age_clock(15_600_000, 0, 0), (1, 15_600_000));
        assert_eq!(age_clock(15_600_000, 0, 1), (1, 15_600_000));
    }

    #[test]
    fn two_halves_read_apart_become_the_count_that_actually_happened() {
        // Ordinary rising: the second low is later, so it is the value.
        assert_eq!(main_count(1, 0x0000_0100, 0x0000_0200), 0x1_0000_0200);
        // The low half rolled over between the two reads, so the high half was
        // captured before the roll and only the first low belongs with it.
        // Keeping the second low here would report a count 4.29 seconds early.
        assert_eq!(main_count(1, 0xFFFF_FFF0, 0x0000_0004), 0x1_FFFF_FFF0);
        // Equal lows: a counter that did not move at all in the interval.
        assert_eq!(main_count(0, 0x1234_5678, 0x1234_5678), 0x1234_5678);
    }

    #[test]
    fn a_jumped_counter_still_prints_its_periodic_reports() {
        // The regression this replaces was `ticks % 1000 == 0`, which only
        // printed when a jump landed exactly on the boundary.
        assert!(period_crossed(990, 1_500, 1_000));
        assert!(period_crossed(0, 1, 1_000) == false);
        assert!(period_crossed(1_000, 1_999, 1_000) == false);
        assert!(period_crossed(1_999, 2_000, 1_000));
        assert!(period_crossed(900, 2_100, 1_000));
        assert!(period_crossed(5, 5, 2_000) == false);
        assert!(period_crossed(0, 0, 0) == false);
    }

    /// Defect B: an idle device printed roughly eight `[PROC]` lines a second.
    ///
    /// The timer tick armed a full process-table dump every two seconds, so a
    /// guest doing nothing at all kept the console -- and the Studio's line
    /// buffer, and anyone measuring `input_lines` -- busy. The table is now
    /// printed when its shape changes and when someone asks for it. This test
    /// holds the task table still and asserts the silence, then changes it one
    /// way at a time to prove the notice is not lost.
    ///
    /// WHY a locally built scheduler: the global one belongs to whichever test
    /// runs next, and the whole point of the rule under test is that output
    /// depends only on whether the table changed.
    #[test]
    fn the_process_table_is_printed_only_when_it_changes() {
        use crate::capability::CapabilityTable;
        use crate::task::process::PROCESS_TEST_LOCK;
        use crate::task::scheduler::Scheduler;
        use crate::task::TaskId;
        use core::sync::atomic::Ordering::Relaxed;

        extern "C" fn idle_entry() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }

        // `spawn` registers a process record in a global table, so this runs
        // serialized with the scheduler's own tests rather than alongside them.
        let _serialized = PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        // Nothing has been shown to the console yet, so the first look shows it.
        PROCESS_TABLE_SHOWN_SIGNATURE.store(u64::MAX, Relaxed);
        assert!(
            process_table_changed(&sched),
            "the kernel loop has never shown a table, so it owes the console one"
        );

        // The idle case, and the defect: the same table sampled again and again
        // must produce exactly nothing, forever.
        for _ in 0..16 {
            assert!(
                !process_table_changed(&sched),
                "an unchanged process table was printed again -- the old two-second \
                 timer is still deciding what the console says"
            );
        }

        // A process appearing is worth telling the console about.
        let probe = sched.spawn(
            "app_quiet_probe",
            idle_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            1,
        );
        assert_ne!(probe, TaskId(0), "the probe task was not spawned");
        assert!(
            process_table_changed(&sched),
            "a new process did not earn a process table"
        );
        assert!(!process_table_changed(&sched), "the same table printed twice");

        // Taking the foreground is a change a reviewer reads; losing a time
        // slice to a co-scheduled neighbour is not. Without this second half,
        // gating on "any field of any line" would put the chatter straight back.
        sched.set_active_foreground_task(Some(probe));
        assert!(
            process_table_changed(&sched),
            "a foreground change did not earn a process table"
        );
        sched.block_task(probe);
        assert!(
            !process_table_changed(&sched),
            "a Ready -> Blocked transition, which every waiting task goes through \
             many times a second, printed the process table"
        );
        sched.unblock_task(probe);
        assert!(
            !process_table_changed(&sched),
            "a Blocked -> Ready transition printed the process table"
        );

        // A process exiting is a change worth printing, even though the task is
        // still in the table waiting to be reaped.
        assert!(
            sched.terminate_child(TaskId(0), probe, 0).is_ok(),
            "the probe task could not be terminated"
        );
        assert!(
            process_table_changed(&sched),
            "a process moving to Zombie did not earn a process table"
        );
    }
}

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

