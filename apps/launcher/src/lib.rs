// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
//! * [`nav`] — the bottom navigation bar's one geometry: the three bands a tap is
//!   answered from and the three marks that are painted inside them.
//! * [`text`] — glyph metrics, so centring and wrapping agree with the rasteriser.
//! * [`search`] — the one index the search runs against, and the ranking seam a
//!   model-backed ranker plugs into.
//! * [`recents`] — the task switcher's card model: what a running task is
//!   called, what it is showing, and what a gesture on its card means.
//!
//! Nothing here talks to the kernel, the package registry or the framebuffer. The
//! shell supplies state; these modules decide where things go and what colour
//! they are. That separation is what makes a theme swappable and a layout
//! testable without a device.
//!
//! WHY [`search`] and [`recents`] are in the library and not in the shell: the
//! shell is `#![no_main]`, so cargo cannot build a test harness for the binary
//! itself. The search's ranking and the switcher's card model are the two
//! pieces of the shell whose behaviour is worth stating in a test rather than
//! only in a frame, and they have to be in the library for that to be possible.

extern crate interos_std;

pub use interos_std::font;
pub mod icons;
pub mod layout;
pub mod nav;
pub mod recents;
pub use interos_std::design::outline_text;
pub mod search;
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