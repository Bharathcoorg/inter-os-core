# Syscall Pointer Safety & Userspace Trust Boundary

This specification defines the memory trust boundary enforced whenever untrusted userspace memory addresses cross the InterOS microkernel ABI (`kernel/src/syscall/user_ptr.rs`).

---

## 1. Required Security Invariants

Every userspace pointer passed into a syscall must satisfy ten fundamental invariants before dereferencing:

1. **Null & Guard-Page Rejection**: Addresses strictly below `0x1000` (`USER_SPACE_START`) are rejected with `UserAccessError::NullOrGuardPage`.
2. **Canonical Range Bounds**: Pointers must reside strictly within lower canonical user space (`addr + len <= USER_SPACE_END` = `0x0000_7FFF_FFFF_FFFF`). Higher-half kernel addresses are rejected with `UserAccessError::KernelAddressExclusion`.
3. **Arithmetic Overflow Protection**: `addr.checked_add(len)` must not overflow `u64`.
4. **Length Ceiling**: Individual transfers are bounded to `MAX_USER_TRANSFER_LEN` (16 MiB) to prevent denial of service or unbounded kernel stalls.
5. **Multi-Page Inspection**: Every 4 KiB page spanned by `[addr, addr + len)` is individually inspected, not merely the initial page.
6. **Hardware Permission Verification**: Every touched page must carry user-accessible mapping flags (`U/S = 1`), and destination buffers must carry writable permissions.
7. **Physical Frame Ownership**: The resolved physical memory frames must belong to the calling task's memory allocation.
8. **Alignment Enforcement**: Typed pointers (`UserPtr<T>`) enforce `addr % core::mem::align_of::<T>() == 0`.
9. **Elimination of Raw Slices**: The kernel syscall dispatcher must **never** construct `core::slice::from_raw_parts` directly over untrusted userspace pointers.
10. **Kernel-Owned Copies**: Handlers read data via kernel-owned buffers (`copy_from_user`) and publish results via verified write copies (`copy_to_user`).

---

## 2. Canonical Primitives (`kernel/src/syscall/user_ptr.rs`)

The memory boundary provides strong, typed safe abstraction wrappers:

- `UserPtr<T>`: Strongly-typed userspace pointer with alignment verification.
- `UserSlice`: Read-only userspace byte slice abstraction.
- `UserSliceMut`: Mutable userspace byte slice abstraction for kernel-to-user output.
- `copy_from_user(dst: &mut [u8], src: UserSlice)`: Validates and copies bytes from user space into kernel buffers.
- `copy_to_user(dst: UserSliceMut, src: &[u8])`: Validates and copies bytes from kernel buffers into user space.
- `read_user_value<T>(ptr: UserPtr<T>)`: Reads a copy of a `Copy` type from userspace.
- `write_user_value<T>(ptr: UserPtr<T>, val: &T)`: Writes a typed value to userspace.
- `read_user_string(slice: UserSlice)`: Reads a bounded, validated UTF-8 string.

---

## 3. Architecture Coverage & Reality Status

- **x86_64 (Fully Exercised & QEMU-Verified)**:
  - Backed by the complete `TaskPageTable::validate_user_range` engine.
  - Page-crossing, permission verification, null exclusion, and frame ownership checks are exercised on every system call.
- **AArch64 & RISC-V 64 (Source-Level Contracts)**:
  - Architecture-specific stubs utilize checked range validation, integer overflow checks, and bounded copy helpers matching static userspace mappings.
  - Runtime verification on physical ARM64/RISC-V silicon remains an experimental bring-up track; these contracts are not represented as physical hardware verified.
