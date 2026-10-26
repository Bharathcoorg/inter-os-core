# Diagnostics and Performance

InterOS Core provides bounded, allocation-free diagnostics and performance measurement primitives suitable for `#![no_std]` bare-metal microkernel execution.

---

## 1. Structured Diagnostics

The microkernel maintains an allocation-free circular diagnostic event ring (`kernel/src/diagnostics.rs`):

- **Bounded Capacity**: Fixed 64-entry circular buffer (`CAPACITY = 64`).
- **Event Structure**:
  - `sequence`: Monotonically increasing 64-bit event counter.
  - `timestamp_ticks`: System timer tick count at event generation.
  - `level`: Explicit severity (`Info`, `Warning`, `Error`, `Critical`).
  - `code`: Structured 16-bit diagnostic domain code.
  - `argument`: 64-bit contextual payload (e.g. fault address, error status).
- **Snapshot ABI**: The diagnostic buffer supports atomic snapshots into caller-provided slices without heap allocation or blocking scheduler execution.
- **Crash Dumps**: Fatal CPU exceptions record machine fault state (`CR2`, registers, stack pointer) into the persistent crash-dump subsystem (`kernel/src/storage/crashdump.rs`).

---

## 2. Low-Overhead Performance Measurement

Performance monitoring is implemented as lightweight, deterministic measurement primitives rather than fabricated benchmark scores (`kernel/src/performance.rs`):

- **Measured Domains**:
  - **Scheduler**: Task context-switch duration and dispatch overhead.
  - **IPC**: Shared-memory ring buffer transfer and futex wake latency.
  - **Frame**: Display compositor frame rendering and scanout present duration.
- **Counter Invariants**:
  - `samples`: Total count of observed execution cycles.
  - `total_ticks`: Cumulative ticks spent across all samples.
  - `worst_ticks`: Maximum observed single-event latency (worst-case tracking).
  - `average_ticks()`: Integer arithmetic average (`total_ticks / samples`).

---

## 3. Truthful Telemetry & Reality Boundary

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md):

- **Measurement Primitives vs. Benchmarks**: Counters record actual tick measurements. InterOS does not manufacture synthetic benchmark figures or publish estimated hardware scores.
- **Hardware vs. Emulated Telemetry**:
  - When running in QEMU, telemetry represents virtual CPU cycles on the host emulator.
  - Physical hardware telemetry requires verified physical sensor/PMIC drivers. When a hardware backend is absent, developer tooling reports `Unavailable` rather than synthetic demonstration values.

---

## 4. Empirical Benchmark Suite & Harnesses

InterOS provides empirical, automated benchmark verification tools under `tools/` with companion test contracts under `tests/`:

- **System & Storage Benchmark (`tools/bench_interos.py`)**:
  - Automatically verifies canonical boot image size (~10.47 MB `inter-os-bios.img`, 10,978,304 bytes), 16 Ring-3 application crates, live kernel heap usage via `[DIAG_STATE]` telemetry, and writes structured results to `target/benchmark_report.json` with fail-closed semantics.
- **Boot Latency Measurement (`tools/bench_boot.py`)**:
  - Measures elapsed monotonic cycles from QEMU process startup to initial scanout present (`Step 5: Initial presentation complete`).
- **Touch-to-Scanout Latency (`tools/bench_tap_latency.py`)**:
  - Correlates hardware touch injection events with display compositor buffer flips, verified by `tests/test_tap_latency_bench.py`.
- **Heap Allocation Invariant Enforcement (`tools/bench_heap.py`)**:
  - Validates that interactive rendering loops, font rasterization, touch event routing, and compositing passes execute with strictly zero heap allocations (`tests/test_heap_bench.py`).
