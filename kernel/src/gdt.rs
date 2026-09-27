//! Global Descriptor Table (GDT) & Task State Segment (TSS)
//!
//! Configures CPU segmentation registers and sets up the Interrupt Stack Table (IST)
//! to provide dedicated stack memory for critical exceptions (e.g. Double Fault),
//! preventing fatal CPU triple faults on kernel stack overflow.

use core::cell::UnsafeCell;
use conquer_once::spin::Lazy;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Index in the Interrupt Stack Table for the Double Fault exception handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the dedicated stack for double fault handling (20 KiB).
const STACK_SIZE: usize = 4096 * 5;

#[repr(transparent)]
struct SyncTss(UnsafeCell<TaskStateSegment>);
unsafe impl Sync for SyncTss {}

static TSS: SyncTss = SyncTss(UnsafeCell::new(TaskStateSegment::new()));

/// Dynamically updates TSS privilege stack table 0 (RSP0) upon task context switch.
pub fn set_tss_rsp0(rsp0: VirtAddr) {
    unsafe {
        (*TSS.0.get()).privilege_stack_table[0] = rsp0;
    }
}

pub struct Selectors {
    pub kernel_code_selector: SegmentSelector,
    pub kernel_data_selector: SegmentSelector,
    pub user_data_selector: SegmentSelector,
    pub user_code_selector: SegmentSelector,
    pub tss_selector: SegmentSelector,
}

static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code_selector = gdt.append(Descriptor::kernel_code_segment());
    let kernel_data_selector = gdt.append(Descriptor::kernel_data_segment());
    let user_data_selector = gdt.append(Descriptor::user_data_segment());
    let user_code_selector = gdt.append(Descriptor::user_code_segment());

    static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
    static mut PRIV_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

    let tss_mut = unsafe { &mut *TSS.0.get() };
    let stack_start = VirtAddr::from_ptr(&raw const STACK);
    let stack_end = stack_start + STACK_SIZE as u64;
    tss_mut.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;

    // Privilege Stack 0 default fallback
    let priv_start = VirtAddr::from_ptr(&raw const PRIV_STACK);
    let priv_end = priv_start + STACK_SIZE as u64;
    tss_mut.privilege_stack_table[0] = priv_end;

    let tss_ref = unsafe { &*TSS.0.get() };
    let tss_selector = gdt.append(Descriptor::tss_segment(tss_ref));
    (
        gdt,
        Selectors {
            kernel_code_selector,
            kernel_data_selector,
            user_data_selector,
            user_code_selector,
            tss_selector,
        },
    )
});

/// Initializes the GDT and loads the Task State Segment (TSS).
#[inline(never)]
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
    use x86_64::instructions::tables::load_tss;
    use core::sync::atomic::{compiler_fence, Ordering};

    let (gdt_ref, selectors) = (&GDT.0, &GDT.1);
    let code_sel = selectors.kernel_code_selector;
    let data_sel = selectors.kernel_data_selector;
    let tss_sel = selectors.tss_selector;

    compiler_fence(Ordering::SeqCst);
    gdt_ref.load();
    unsafe {
        CS::set_reg(code_sel);
        DS::set_reg(data_sel);
        ES::set_reg(data_sel);
        SS::set_reg(data_sel);
        load_tss(tss_sel);
    }
    compiler_fence(Ordering::SeqCst);
}

/// Returns the static GDT segment selectors.
pub fn selectors() -> &'static Selectors {
    &GDT.1
}

