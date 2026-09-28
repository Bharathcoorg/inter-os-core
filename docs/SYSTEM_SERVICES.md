# InterOS System Services

InterOS keeps hardware-facing policy outside the microkernel's application model. First-party services run as isolated Ring-3 processes, receive only the capabilities required by their role, and report lifecycle state through the service manager.

## Service set

| ID | Service | Responsibility | Lifecycle |
| --- | --- | --- | --- |
| 1 | displayd | display surface and scanout boundary | restart on failure |
| 2 | inputd | normalized touch/input delivery | restart on failure |
| 3 | storaged | persistent filesystem/storage boundary | restart on failure |
| 4 | netd | network device polling and sockets | restart on failure |
| 5 | powerd | battery, power and DVFS boundary | restart on failure |
| 6 | modemd | cellular/baseband boundary | restart on failure |
| 7 | audiod | audio feedback/stream boundary | restart on failure |
| 8 | camerad | camera control/capture boundary | restart on failure |
| 9 | compositor | window composition and display integration | restart on failure |
| 10 | launcher | application launch and shell integration | restart on failure |
| 11 | sensord | IMU/light/proximity sensor boundary | restart on failure |

## Security boundary

Services do not receive unrestricted hardware authority by default. Hardware control APIs use object-scoped `SystemControl` capabilities for power, telephony, audio, camera, and sensor domains. Generic `SystemControl` remains reserved for explicitly privileged kernel paths.

The storage service has separate filesystem read/write authority and bounded raw-block read access. Network service authority is split between socket creation, transmit, and receive capabilities. Display authority is scoped to the display object.

Signed service packages use dedicated capability identifiers for these scoped hardware domains. Unknown or unsupported service capabilities remain rejected.

## Lifecycle

PID 1 owns startup, recovery, and shutdown. The kernel service registry owns authoritative service identity, parent ownership, dependency checks, heartbeat validation, restart backoff, and generation tracking.

A service heartbeat is accepted only when the calling task matches the registered service task. Stale or terminated workers become failed and must pass the bounded recovery path before a new instance is started.

A running dependency cannot be stopped while a registered dependent service is still running. Recovery verifies that a failed service task is owned by the requesting PID 1 instance before terminating and reaping it.

## Resource isolation

Each first-party service receives a process-level budget for threads, open files, IPC endpoints, and userspace address space. These limits are applied immediately after process registration and before the service is published as running.

Service diagnostics expose lifecycle state, generation, heartbeat age, and cumulative failure count through the existing bounded service-list ABI.

## Verification boundary

This document defines source-level contracts. It does not claim a fresh kernel build, CI execution, QEMU boot, repeated runtime soak, or physical-device validation.
