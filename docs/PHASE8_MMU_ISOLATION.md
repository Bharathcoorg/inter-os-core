# Phase 8 — MMU Isolation, ELF Loading & Address-Space Accounting

Phase 8 makes the x86_64 userspace memory boundary a kernel-enforced invariant.

## Address-space contract

Each Ring-3 process receives an independent PML4. Only the kernel's higher-half PML4 entries are mirrored into a userspace address space; the lower half is owned exclusively by that process.

The TaskPageTable owns its PML4 frame, intermediate page-table frames, user leaf frames, and the mapped userspace byte counter. When the final reference is released, owned frames are returned to the physical-frame allocator. If the active CR3 references the address space, teardown first switches back to the kernel page table.

## ELF contract

The direct ELF loader accepts only 64-bit little-endian x86_64 ELF, accepts ET_EXEC and ET_DYN images, rejects PT_INTERP dynamic linkers, validates program-header and segment bounds, rejects overlapping load ranges, rejects load segments overlapping the user stack/guard, requires the entry point to lie in an executable segment, allocates and zeroes physical frames directly, maps code executable/read-only, maps writable data and stacks NX, and leaves one unmapped stack guard page.

The mapping primitive independently rejects writable executable user mappings, so W^X does not depend solely on ELF metadata validation.

## Process resource contract

ResourceLimits.max_address_space_bytes is enforced against the process's mapped user virtual-memory footprint. ELF mappings and the initial stack establish the process footprint. New userspace thread stacks reserve their footprint before mapping. Failed thread creation rolls back the reservation. Exiting userspace threads unmap their dedicated stack and release its budget. Tightening a process limit below current address-space usage is rejected.

## Validation boundary

`tests/test_phase8_mmu_contract.py` checks the source-level isolation, W^X, ELF, teardown, and resource-accounting contracts.

The repository has not been claimed as freshly QEMU-boot-validated, CI-tested, or physically validated by this phase unless separately evidenced.
