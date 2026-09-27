# Diagnostics and Performance

InterOS keeps diagnostics and performance measurement bounded and architecture-neutral.

## Diagnostics

The kernel diagnostic ring stores a fixed number of structured events containing sequence number, tick timestamp, severity, code, and an opaque argument. It is suitable for boot diagnostics, service failures, and post-failure inspection without allocating an unbounded log.

Crash dumps remain the authoritative fatal-state artifact and include processor fault registers plus integrity metadata.

## Performance

Performance counters track samples, accumulated ticks, and worst-case latency for scheduler, IPC, and frame paths. They are measurement primitives, not fabricated hardware benchmarks. Device Studio may expose them only when the guest/backend provides real measurements.

## Hardware truth

QEMU and physical-device telemetry must remain distinguishable. An unavailable hardware backend must report unavailable rather than inventing values.
