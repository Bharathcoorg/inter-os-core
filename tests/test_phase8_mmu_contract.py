#!/usr/bin/env python3
"""Phase 8 MMU, ELF, W^X, and address-space resource contracts.

Static validation only; this test does not claim QEMU, CI, or physical-device
execution.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    memory = (ROOT / "kernel/src/memory.rs").read_text()
    user = (ROOT / "kernel/src/task/user.rs").read_text()
    process = (ROOT / "kernel/src/task/process.rs").read_text()
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text()

    must(memory, "for i in 256..512", "kernel-only PML4 mirroring")
    must(memory, "USER_ACCESSIBLE", "user mapping permission boundary")
    must(memory, "W^X violation: writable user mapping must be non-executable", "mapping-level W^X")
    must(memory, "user_mapped_bytes", "address-space footprint accounting")
    must(memory, "physical frame already owned by address space", "frame alias rejection")
    must(memory, "impl Drop for TaskPageTable", "address-space frame reclamation")
    must(memory, "switch_to_kernel_page_table()", "safe active-address-space teardown")

    must(user, "load_user_elf_into_page_table", "direct per-process ELF loader")
    must(user, "PT_INTERP dynamic linkers rejected", "static ELF policy")
    must(user, "W^X security violation", "ELF W^X enforcement")
    must(user, "Stack guard address underflow", "stack guard validation")
    must(user, "USER_STACK_SIZE", "dedicated user stack")
    must(user, "set_address_space_usage", "process address-space accounting")
    must(user, "reserve_address_space", "thread stack budget reservation")

    must(process, "address_space_bytes: u64", "process address-space counter")
    must(process, "max_address_space_bytes", "address-space resource limit")
    must(process, "reserve_address_space", "address-space reservation API")
    must(process, "release_address_space", "address-space release API")
    must(process, "limits below current address-space usage", "safe limit tightening")

    must(scheduler, "unmap_user_thread_stack", "thread stack reclamation")
    must(scheduler, "release_address_space", "thread address-space release")
    must(scheduler, "switch_to", "per-task CR3 switching")

    print("phase 8 MMU/address-space contract: PASS")

if __name__ == "__main__":
    main()
