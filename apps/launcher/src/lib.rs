#![no_std]
//! InterOS launcher presentation layer.
//!
//! WHY this exists as a library: the shell is `#![no_main]`, so cargo cannot build
//! a test harness for the binary itself, and an integration test would still drag
//! the bare-metal binary into the link. Putting the presentation modules in a lib
//! target lets them be tested for real on the host, against the same source the
//! firmware links.
//!
//! Contents:
//!
//! * [`theme`] — the visual language as data: colours, type scale, radii, spacing.
//! * [`layout`] — geometry computed from counts, so the shell adapts to whatever
//!   apps and options exist rather than asserting a fixed set.
//! * [`text`] — glyph metrics, so centring and wrapping agree with the rasteriser.
//!
//! Nothing here talks to the kernel, the package registry or the framebuffer. The
//! shell supplies state; these modules decide where things go and what colour
//! they are. That separation is what makes a theme swappable and a layout
//! testable without a device.

pub mod layout;
pub mod text;
pub mod theme;

/// WHY this is test-only: the crate is `#![no_std]` because it runs on bare metal,
/// where `std` does not exist. The test harness, however, is a host binary that
/// needs `std` for its own machinery. Linking it in under `cfg(test)` keeps the
/// firmware image free of it.
#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;