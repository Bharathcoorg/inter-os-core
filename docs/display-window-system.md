# Display and window system

Phase 7 establishes the reusable display/window architecture above the
hardware-facing KMS adapter.

## Contract

A window is represented by an owned display surface. The GUI `WindowManager`
and KMS surface registry share the same stable window ID; KMS creation,
geometry, focus and destruction are synchronized with the GUI registry. The
kernel remains the authority for:

- window ownership and lifecycle;
- geometry, z-order and visibility;
- hit-testing and focused-window selection;
- pointer/touch input routing;
- surface destruction when the owning task exits;
- bounded surface memory and surface count;
- metadata snapshots for compositor/shell tooling.

Applications do not receive direct framebuffer ownership. Window mutation is
performed through capability-checked display service calls.

## Input isolation

Normalized touch/gesture events are routed to the focused window owner's
task. Tap-like events perform authoritative hit-testing; pan/pinch gestures
retain the captured focused owner instead of switching applications mid-gesture. Application input polling removes only events addressed to the calling
task. The display/input service retains its separate service queue.

This prevents an unrelated application from consuming another application's
focused-window input.

## Window lifecycle

The window contract covers:

1. create an owned surface/window;
2. update owned pixels;
3. update owned geometry and visibility;
4. participate or opt out of input hit-testing;
5. focus the topmost eligible window at a display coordinate;
6. inspect window metadata;
7. destroy the window;
8. automatically reclaim windows when the owner task exits.

## Current validation boundary

The repository contains static contract tests for ownership, geometry,
hit-testing, focused input routing and lifecycle cleanup.

This phase has not been claimed as freshly QEMU-boot-validated, CI-validated,
or physically validated unless separately evidenced.
