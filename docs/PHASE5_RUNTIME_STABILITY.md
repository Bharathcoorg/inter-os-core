# Phase 5 — End-to-End Runtime Stability

Phase 5 hardens the InterOS vertical slice for repeated application lifecycle use.

## Runtime contract

The supported QEMU development lifecycle is:

1. cold boot
2. PID1 initialization
3. deterministic system-service bootstrap
4. launcher presentation
5. application launch
6. display/input delivery
7. application exit
8. IPC/filesystem/network/event/surface cleanup
9. child reaping and process accounting
10. launcher recovery
11. repeated application cycles
12. reset/reboot without stale task/resource state

## Stability invariants implemented

### Input

The fixed-capacity input queue remains non-allocating, but overflow is now observable through a cumulative dropped-event counter rather than silently disappearing.

### Events

The event reactor no longer has a fixed 16-waiter wakeup cap. Every matching waiter is considered for wakeup. If the scheduler lock is temporarily unavailable, waiter state is retained so a notification cannot strand a task.

Timer deadline wakeups use the same fail-safe rule.

### Process accounting

Process validation now reconciles each process record's thread_count with the actual thread table and rejects orphan thread bindings without a live process record.

### Task teardown

The common termination path releases IPC ownership, filesystem ownership, event subscriptions, network ownership, and application display surfaces, then reparents live children to primordial init.

Zombie tasks remain available for the owning parent to reap.

### Display/surface lifecycle

Client surfaces are bounded to 32 simultaneous surfaces and maximum 1280x720 pixels per surface, with checked surface-ID allocation and explicit allocation failure propagation.

## Existing end-to-end coverage

The repository's interactive lifecycle test covers cold boot, PID1/services, launcher presentation, application interaction, Back/exit, recent-app recovery, card dismissal, quick controls, and notification flows.

The new test_phase5_runtime_stability.py contract test ties those runtime flows to the kernel cleanup and accounting invariants.

## Verification boundary

Phase 5 is source/contract complete.

This work does not claim fresh CI, kernel compilation, QEMU execution, long-run 10+ cycle execution, or physical-device validation. Those remain evidence-producing validation steps rather than being represented as completed merely by static code changes.
