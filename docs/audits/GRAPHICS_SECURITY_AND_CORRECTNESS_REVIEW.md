# ADVERSARIAL SECURITY, CORRECTNESS, AND ARCHITECTURAL REVIEW
**Subsystem:** Graphics, DRM/KMS Compositor, Framebuffer HAL, and Interaction Pipeline  
**Reviewer:** Agent H — Independent Security, Correctness, and Architecture Review Specialist  
**Target Branch:** `master` (InterOS Core)  
**Status:** **CRITICAL RISKS IDENTIFIED — HARD SAFETY REQUIREMENTS DEFINED**

---

### EXECUTIVE SUMMARY & VERDICT

An exhaustive adversarial examination of `kernel/src/drivers/display_kms.rs`, `kernel/src/framebuffer.rs`, `kernel/src/syscall/mod.rs`, `kernel/src/capability/mod.rs`, `kernel/src/task/user.rs`, `kernel/src/syscall/user_ptr.rs`, and associated security contract suites has been performed.

While the existing codebase demonstrates disciplined `#![no_std]` and `#![no_main]` conventions, **the current graphics and windowing architecture contains severe security and privilege boundary vulnerabilities that violate InterOS Core's core tenets: Zero Ambient Authority, Process Isolation, and Panic Resilience.**

Most alarmingly:
1. **Total Capability Boundary Collapse**: To allow applications to draw, `CapabilityType::FrameBufferDirect` (intended strictly for physical display hardware manipulation) has been ambiently granted to **every single application** in `kernel/src/task/user.rs`. Consequently, any unprivileged app can reprogram hardware scanout modes, rotate the physical display, force screen flips, and overwrite display z-ordering.
2. **Denial-of-Service Process Termination**: Any untrusted app can allocate large surfaces to exhaust the 48 MiB surface budget and trigger the kernel eviction loop in `sys_surface_create`, forcing the kernel to terminate arbitrary background tasks without possessing `CapabilityType::ProcessKill`.
3. **Touch Hijacking & Phishing Overlays**: Applications can set arbitrary `z_order` values up to `u32::MAX` via `sys_window_set_geometry`, positioning transparent or spoofed windows above system overlays and stealing all user touch input.
4. **Ring-0 Blast Radius & Spinlock Inversion**: The DRM/KMS compositor runs thousands of lines of software rasterization, alpha blending, scaling, and dirty-band tracking inside Ring 0 under a raw `spin::Mutex`. Holding this spinlock across 16 MiB user memory transfers causes severe latency spikes and scheduler deadlocks.
5. **Pseudo-Contract Verification**: Several existing "truth contract" tests (e.g. `test_virtio_gpu_contract.py`, `test_surface_composite_contract.py`, `test_display_contract.py`) do not execute guest code or verify hardware behavior; they merely perform string searches on Rust source code, masking fundamental security violations.

---

### SECTION 1: MEMORY SAFETY & INTEGER ARITHMETIC

#### 1.1 Stride, Pitch, and Dimension Calculations in Scanout Blits
* **Location:** `kernel/src/framebuffer.rs:1977-2200` (`present_argb8888_offset`), `kernel/src/drivers/display_kms.rs:2225-2287`.
* **Vulnerability Analysis:**
  1. **Unchecked Scanout Pointer Arithmetic:** In `present_argb8888_offset`:
     ```rust
     let target_y = off_y + dy + y_offset;
     let row_start_ptr = ptr.add(target_y * stride_bytes + off_x * bpp);
     ```
     `ptr` is a raw pointer (`*mut u8`) to physical video RAM. The calculation `target_y * stride_bytes + off_x * bpp` lacks an explicit boundary assertion against the mapped framebuffer size (`FB_COVERED_BYTES` or `info.byte_len`). If `y_offset` is non-zero or if geometry updates lead to an uncoordinated `dy1`, `row_start_ptr` strides directly past mapped video RAM into unmapped physical memory or adjacent kernel direct-mapped pages, causing fatal kernel page faults.
  2. **Signed Coordinate Underflow / Wrap-Around:** In `display_kms.rs:2225-2236`:
     ```rust
     let src_y = (y - sy) as usize;
     let src_y_offset = src_y * (surface.width as usize);
     let src_x = (x - sx) as usize;
     let src_idx = src_y_offset + src_x;
     let src_pixel = surface.pixels[src_idx];
     ```
     When a surface is positioned at negative coordinates (`sx < 0` or `sy < 0`), `(x - sx)` evaluates a subtraction between signed integers. If an adversarial application sets `sx = i32::MIN`, `(x - sx)` overflows signed 32-bit arithmetic. When cast to `usize`, this yields a massive index `0xFFFF_FFFF_xxxx_xxxx`. In release builds without overflow checks, this triggers immediate kernel panic on slice indexing or out-of-bounds reads.
  3. **32-Bit Multiplication Truncation:** In `DrmSurface::new`:
     ```rust
     let size = (width as usize) * (height as usize);
     ```
     On 32-bit architectures (AArch32, RV32), `width * height` can overflow `usize` if `width` and `height` are unchecked before entry, resulting in undersized buffer allocation and subsequent buffer overflow during pixel blits.

#### 1.2 Surface Buffer Allocations & Heap Fallback Panic
* **Location:** `kernel/src/drivers/display_kms.rs:350-395` (`SurfaceBuffer::new`).
* **Vulnerability Analysis:**
  - `SurfaceBuffer::new` attempts to allocate physically contiguous frames via `allocate_dma_frames(pages)`. If physical frames are exhausted, it silently falls back to:
    ```rust
    let mut heap_vec = vec![default_color; pixel_count];
    ```
  - In a constrained 64 MiB kernel heap, allocating up to `MAX_SURFACE_BYTES` (48 MiB) on the heap deprives the kernel of memory for page tables, task descriptors, and IPC mailboxes.
  - In Rust `#![no_std]`, an allocation failure in `alloc::vec!` triggers an immediate kernel panic, causing an unrecoverable CPU halt. **An untrusted client surface request must NEVER be able to panic the kernel.**

#### 1.3 Userspace Buffer Passing & TOCTOU Races
* **Location:** `kernel/src/syscall/user_ptr.rs:174-196` (`copy_to_kernel_typed`).
* **Vulnerability Analysis:**
  - `UserSlice::copy_to_kernel_typed` calls `validate_current_user_buffer` to verify page mapping and permissions, then executes `core::ptr::copy_nonoverlapping` from raw userspace address `self.addr`.
  - **TOCTOU Race Condition:** If a multithreaded userspace process unmaps the memory range (`sys_mem_unmap`) concurrently between the validation check and `copy_nonoverlapping`, the kernel execution traps on a user-space Page Fault while executing in Ring 0. Without an in-kernel exception table (`extable`) fixup handler, this panics and halts the OS.

---

### SECTION 2: PRIVILEGE & CAPABILITY BOUNDARIES

#### 2.1 Total Collapse of Zero Ambient Authority
* **Location:** `kernel/src/syscall/mod.rs:4705-4719`, `kernel/src/task/user.rs:697-2389`.
* **Critical Finding:**
  - `require_display_service(cap_slot)` checks for `CapabilityType::FrameBufferDirect, Rights::WRITE`.
  - Because creating and painting a window calls `sys_surface_create` and `sys_surface_write`, which gate on `require_display_service`, **all 17 native applications in `kernel/src/task/user.rs` are granted `CapabilityType::FrameBufferDirect`!**
  - This completely destroys privilege separation. Holding `FrameBufferDirect` allows any untrusted client application to:
    1. Call `sys_display_set_mode` to reprogram the physical CRTC/DISPI hardware resolution and refresh rate (`syscall/mod.rs:5014`).
    2. Call `sys_display_set_orientation` to physically flip or invert the display (`syscall/mod.rs:5045`).
    3. Call `sys_surface_flip` to force scanout commits asynchronously (`syscall/mod.rs:4979`).
    4. Call `sys_window_set_geometry` to move and restyle other surfaces.
  - **Violation of Zero Ambient Authority:** Applications should only hold authority over their own isolated surface canvases. Granting physical framebuffer controller capability to every user-space application is a fatal architectural defect.

#### 2.2 Arbitrary Z-Order Manipulation & Touch Hijacking (Clickjacking)
* **Location:** `kernel/src/drivers/display_kms.rs:1427-1445` (`set_surface_geometry_owned`), `kernel/src/gui/window.rs:138-144` (`hit_test`).
* **Vulnerability Analysis:**
  1. `sys_window_set_geometry` accepts an unvalidated `z: u32` parameter. In `set_surface_geometry_owned`, `s.z_order = z;` is assigned without bounds checking or clamping!
  2. While `create_surface` attempts to clamp regular apps between `200..1000`, `set_surface_geometry_owned` allows any task to update its z-order to `10_000` or `u32::MAX`.
  3. In `WindowManager::hit_test`:
     ```rust
     let target = self.windows.iter()
         .filter(|w| w.visible && w.kind != WindowKind::SystemOverlay && w.rect.contains(x, y))
         .max_by_key(|w| w.z)
         .map(|w| (w.id, w.owner));
     ```
  4. An unprivileged application can place a transparent 720x1440 window at `z = 9999`. It will receive **100% of all touch events**, intercepting PIN entries, lock screen interactions, and credentials without the user or kernel detecting the tap interception.

#### 2.3 Denial-of-Service Process Termination via Memory Eviction
* **Location:** `kernel/src/syscall/mod.rs:4734-4808` (`sys_surface_create`), `kernel/src/drivers/display_kms.rs:1046-1052` (`evictable_app_owner`).
* **Vulnerability Analysis:**
  - When `sys_surface_create` is invoked, if `kms.can_afford_surface(pixels)` returns false, the kernel executes an eviction loop:
    ```rust
    let Some((victim, held)) = candidate else { break };
    ...
    SCHEDULER.lock().terminate_task(TaskId(victim), DrmKmsController::EXIT_CODE_MEMORY_RETIRE);
    ```
  - An untrusted client can repeatedly allocate maximum-sized surfaces (1080x2160x4 = ~9.3 MB) until memory is depleted, triggering the kernel to execute `terminate_task` on background tasks.
  - **Bypassing `CapabilityType::ProcessKill`:** The calling task terminates other tasks without possessing process kill capabilities.

#### 2.4 Pixel Spying and Missing `CapabilityType::SurfaceShare`
* **Location:** `kernel/src/capability/mod.rs:100-110`, `kernel/src/syscall/mod.rs:4859-4870`.
* **Vulnerability Analysis:**
  - `sys_surface_capture` checks only global `CapabilityType::SurfaceCapture` by type. It does not check object ID, ownership, or whether the source application consented to sharing its surface.
  - The codebase has **no fine-grained `CapabilityType::SurfaceShare`**. Once a task holds `SurfaceCapture`, it can capture every surface on the system (e.g. Vault PIN pads, private messages, browser sessions).
  - Furthermore, `map_to_task` in `SurfaceBuffer` does not check caller capabilities, relying on the caller to enforce authorization.

---

### SECTION 3: CONCURRENCY, LOCKING & PANIC SAFETY

#### 3.1 Unbounded Spinlocks Held Across Multi-Megabyte Operations
* **Location:** `kernel/src/syscall/mod.rs:2813-2893` (`Syscall 73: SurfaceWrite`), `kernel/src/drivers/display_kms.rs:1596-2294` (`composite`).
* **Vulnerability Analysis:**
  1. In `SyscallNumber::SurfaceWrite`:
     ```rust
     let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
     ...
     user_slice.copy_to_kernel_typed(&mut surface.pixels[offset..end]);
     ```
     `DRM_KMS.lock()` is held while copying up to 16 MiB (`4_194_304` pixels) from userspace memory into the kernel surface buffer.
  2. While `DRM_KMS` is held, any other core or task attempting to invoke `sys_surface_flip`, `sys_window_list`, or `sys_surface_create` spins continuously.
  3. **Priority Inversion / Deadlock:** If the task performing `SurfaceWrite` is preempted or yields while holding `DRM_KMS.lock()`, and a higher-priority task attempts to acquire `DRM_KMS.lock()`, the system wedges permanently. The kernel comment in `syscall/mod.rs:7800` explicitly confesses that this exact condition has previously frozen the guest.

#### 3.2 Non-Blocking Lock Safety in Panic & Interrupt Contexts
* **Location:** `kernel/src/panic.rs`, `kernel/src/main.rs:492-535`, `kernel/src/interrupts.rs:780-997`.
* **Findings:**
  - The panic handler in `kernel/src/main.rs:508-515` correctly avoids blocking `.lock()` calls by using `kernel::allocator::try_heap_stats()` and `SCHEDULER.try_lock()`.
  - However, `PICS.lock()` in `kernel/src/interrupts.rs:793, 928` is a blocking spinlock inside interrupt service routines (ISRs). If an interrupt triggers while non-interrupt code holds `PICS.lock()`, self-deadlock occurs.
  - Neither `DRM_KMS.lock()` nor `FRAMEBUFFER_WRITER.lock()` disables interrupts during acquisition. An interrupt handler attempting to interact with the display would result in a fatal deadlock.

---

### SECTION 4: ARCHITECTURAL CRITIQUE: USERSPACE VS KERNEL COMPOSITOR

A core architectural question for this overhaul is whether the compositor should reside in the kernel or be moved to a userspace system daemon.

| Evaluation Vector | Model A: Kernel Compositor (Current Architecture) | Model B: Userspace Compositor Daemon (Modern Microkernel) |
| :--- | :--- | :--- |
| **TCB Size & Blast Radius** | **CRITICAL RISK:** ~4,000 lines of complex blit, alpha blend, scaling, and damage arithmetic live in Ring 0. Any arithmetic bug or out-of-bounds pointer crashes the entire kernel. | **SUPERIOR:** Compositor runs in Ring 3 (userspace). A crash/panic is isolated to the compositor process and caught by the supervisor (`init`), allowing automatic recovery without CPU reset. |
| **Capability Hygiene** | **FAILED:** Kernel must grant `FrameBufferDirect` to all client apps to permit surface operations, demolishing Zero Ambient Authority. | **SUPERIOR:** Only the compositor daemon holds display hardware capabilities (`FrameBufferDirect`). Client apps hold strictly scoped `SurfaceHandle` / `ShmProducer` tokens. |
| **Locking & Preemption** | **POOR:** Large composition passes and memory copies hold kernel spinlocks, creating latency spikes and stalling scheduler ticks. | **SUPERIOR:** Blitting and compositing occur in userspace threads subject to standard preemption without blocking kernel data structures. |
| **Zero-Copy Performance** | **MODERATE:** Relies on kernel heap buffers or DMA physical frame pools, often requiring synchronous syscall copies. | **OPTIMAL:** Ping-pong double buffering in shared physical pages (`shm`). Zero-copy page flips with atomic fence handoffs. |
| **Attack Surfaces Introduced** | Kernel memory corruption, Ring-0 panic DoS, ambient display control leakage. | Shared memory race conditions (client writing while compositor reads), IPC message flooding/spoofing, shm handle exhaustion. |

#### Rigorous Threat Analysis for Userspace Compositor Model:
If the overhaul transitions to a userspace compositor daemon, the following attack surfaces **MUST** be mitigated:
1. **Shared Memory Concurrent Mutation (Tearing & Malformed State):** The compositor must never assume client shared memory is immutable during compositing. Reading directly from client-writable memory during blending can lead to TOCTOU color/alpha inconsistencies. The kernel must implement atomic ping-pong buffer latching or enforce page-table write-protect toggles during scanout.
2. **IPC Mailbox Flooding & Spoofing:** Malicious apps could flood the compositor IPC queue with fake `Commit` commands. The IPC subsystem must enforce message sender verification (authenticating caller `TaskId` via kernel-stamped IPC metadata, not userspace header fields).
3. **Z-Order Clamping in Broker:** The compositor must strictly disallow clients from declaring arbitrary z-indices. Application z-indices must be partitioned into an isolated band (e.g. `100..499`), reserving `500+` exclusively for system dialogues, lock screens, and the launcher shell.

---

### SECTION 5: PLATFORM TRUTH & VERIFICATION INTEGRITY

The Platform Truth Contract states: *"Never fake hardware telemetry, never claim subpixel AA when the algorithm is 4-bit grayscale coverage, and never introduce mock drivers claiming to be physical silicon."*

#### 5.1 Audit of Fragile / Pseudo-Contract Tests
An audit of tests in `tests/` revealed tests that pass purely by string-scraping source code rather than testing actual guest execution or hardware behavior:

1. **`tests/test_virtio_gpu_contract.py`**:
   - Reads `kernel/src/drivers/virtio_gpu.rs`, `kernel/src/memory.rs`, and `kernel/src/drivers/display_kms.rs` as text files and asserts string needles (e.g. `assert "VIRTIO_GPU_CMD_GET_DISPLAY_INFO" in gpu_src`).
   - It executes **zero** VirtIO commands, initializes no ring buffers on QEMU, and does not test device negotiation with an actual virtual device. A non-functional mock file containing these string constants passes 100%.
2. **`tests/test_surface_composite_contract.py`**:
   - Uses regex to parse `display_kms.rs` and `apps/launcher/src/main.rs` (checking for `!surface.frame_complete`, `520, 160`). It verifies source formatting rather than rendered frame correctness.
3. **`tests/test_display_contract.py`**:
   - Specifically asserts `must(services, "cap_type: CapabilityType::FrameBufferDirect, rights: Rights::WRITE, object_id: 1", "displayd-only framebuffer capability")`.
   - **False Verification:** It claims `FrameBufferDirect` is "displayd-only" while ignoring `kernel/src/task/user.rs`, where `FrameBufferDirect` is distributed to 17 other applications! The test provides false assurance.

#### 5.2 Simulated Silicon & Fake Refresh Claims
- **Location:** `kernel/src/drivers/display_kms.rs:162-171` (`frame_budget_ns`), `kernel/src/drivers/virtio_gpu.rs:1826-1837`.
- **Findings:**
  - `DrmKmsController` allows setting `refresh_hz` to `120`, `144`, `240`, and `360` Hz and computes theoretical budgets.
  - However, in `launch_studio.ps1`, QEMU is launched with standard Bochs `VGA` (`-vga none -device VGA,edid=on,xres=720,yres=1440,vgamem_mb=32`).
  - Bochs DISPI hardware has **no programmable pixel clock or vblank interrupt**. Claiming 120Hz/240Hz frame presentation over DISPI is simulated fiction. Telemetry reporting these refresh rates must honestly report them as "Software Target Pacing" rather than physical hardware sync.

---

### SECTION 6: MANDATORY HARD SAFETY & ARCHITECTURAL REQUIREMENTS

For the proposed graphics and interaction overhaul to be accepted into `master`, the engineering implementation **MUST** satisfy the following hard invariants:

1. **Strict Capability De-Escalation (Eliminate `FrameBufferDirect` from Apps):**
   - Strip `CapabilityType::FrameBufferDirect` from all client applications in `kernel/src/task/user.rs`.
   - Introduce an isolated, non-hardware capability: `CapabilityType::SurfaceCanvas` (or migrate to userspace compositor IPC).
   - Only `displayd` / `compositor` may hold `CapabilityType::FrameBufferDirect`.
   - Gating syscalls `sys_display_set_mode`, `sys_display_set_orientation`, and `sys_surface_flip` must reject any caller that is not the authorized display service.
2. **Z-Order Isolation & Window Authority:**
   - Enforce strict z-index partitioning in the kernel / compositor broker:
     - `0..99`: Background / Wallpaper
     - `100..499`: Client Application Surfaces (clamped automatically by kernel)
     - `500..899`: System Overlays & Dynamic Island (privileged)
     - `900..1000`: Lock Screen & Emergency Alerts
   - Disallow clients from declaring modal status or setting z-indices `>= 500`.
3. **Surface Budget Quotas Without Ambient Process Termination:**
   - Eliminate the unprivileged process eviction loop in `sys_surface_create`.
   - When the surface memory limit (`MAX_SURFACE_BYTES`) is reached, `sys_surface_create` must return `Err(SyscallResult::OutOfMemory)`.
   - If background eviction is required, it must be performed solely by an attested system resource supervisor via explicit `CapabilityType::ProcessKill`.
4. **Locking & Preemption Guarantees:**
   - Remove blocking `memcpy` operations from under `DRM_KMS.lock()`.
   - Implement bounded try-locks or transfer pixel chunks outside the global driver lock.
   - For panic contexts, maintain `.try_lock()` and guarantee that no display mutex can stall emergency serial crash logging.
5. **Memory Safety & Extable Fixups:**
   - Replace all unchecked arithmetic in scanout and surface blitting with `checked_` or `saturating_` arithmetic.
   - Guard against negative coordinate underflow (`x - sx`) before casting to `usize`.
   - Implement kernel exception recovery (`extable`) for all userspace memory copies in `user_ptr.rs` to prevent user-induced page faults from crashing Ring 0.
6. **Platform Truth Contract Compliance:**
   - Replace string-scraping Python tests with executable behavioural integration contracts running in QEMU.
   - Clearly label all display refresh telemetry as software-pacing intervals unless hardware VirtIO-GPU / DRM vblank interrupts are physically asserted.

---
*Report submitted to Parent Agent for engineering evaluation and architectural enforcement.*
