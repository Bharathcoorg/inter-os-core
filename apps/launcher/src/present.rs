// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The two decisions a shell frame is made of, stated once so they cannot drift.
//!
//! WHY THIS MODULE EXISTS
//!
//! The shell presents by calling `std::surface_flip`, and the kernel's flip
//! composites *every* surface and copies the changed scanout rows: it is a claim
//! about the whole panel, not about the caller. Two separate defects came from
//! that claim being made by code that had not drawn anything.
//!
//! * A beat that drew no pixels and wrote no pixels still flipped. `render_view`
//!   already presents (it calls `surface_flip_display` itself), and roughly every
//!   call site in the shell presents a *second* time right after it, so each
//!   repaint ended with one present whose only claim was "the surface is current"
//!   — over an app that had already presented its own frame into the same scanout.
//! * The view a tap is answered from, `state.current_view`, was written by input
//!   handlers that never painted. Launching an app set it to `HomeScreen` while
//!   the surface still held the app drawer, so the next tap hit-tested the home
//!   grid against drawer pixels and nothing launched.
//!
//! These are the same invariant seen from two sides: **the surface you painted
//! and the view you hit-test are decided together, and a frame is presented only
//! when a beat actually put bytes into it.** One mechanism enforces both, rather
//! than two patches for two symptoms: [`PresentGate`] decides whether a beat may
//! present at all, and [`PaintedView`] records which view the surface actually
//! holds so a partial repaint can never mix two views' rows.
//!
//! WHY THIS IS IN THE LIBRARY AND NOT IN THE SHELL BINARY
//!
//! `apps/launcher` is `#![no_main]`, so cargo cannot attach a test harness to the
//! binary (`apps/launcher/Cargo.toml`, the `[[bin]] test = false` line), and the
//! same reasoning that put [`crate::search`] and [`crate::recents`] in the library
//! applies here: this is the shell's behaviour that is worth stating in a test
//! rather than only in a frame. Both types below are pure state machines with no
//! syscall behind them, which is what makes the invariant checkable on the host
//! against the exact source the firmware links.
//!
//! NOTHING HERE TALKS TO THE KERNEL. The shell supplies the calls; these decide.

/// Whether a beat is allowed to present, tracked by counting the surface writes
/// it actually performed.
///
/// WHY a count and not a boolean: one band repaint issues one `surface_write` per
/// 32,768-pixel chunk, so a real frame performs several. Counting means the gate
/// can also report *how much* a beat did, which is the number the FRAMEBREAK line
/// needs in order to stop claiming `px = 1,036,800` for a 60-row band.
///
/// WHY `force_next` exists: a present is not always about this surface's bytes.
/// The volume HUD is a separate window, and retiring it changes the composited
/// panel without a single write into the shell's surface. That beat genuinely
/// must present, so the exception is one named call rather than a hole in the
/// rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentGate {
    writes: u32,
    pixels: usize,
    forced: bool,
}

impl PresentGate {
    /// A gate with nothing owed: the state at the start of every beat.
    pub const fn new() -> Self {
        PresentGate {
            writes: 0,
            pixels: 0,
            forced: false,
        }
    }

    /// Records that this beat put `pixels` pixels into the surface.
    ///
    /// WHY the caller reports the count instead of the gate inferring it: only the
    /// writer knows how wide the run was, and the shell writes the surface in
    /// chunks of a fixed capacity, so a count of calls would be a count of chunks.
    pub fn note_write(&mut self, pixels: usize) {
        if pixels == 0 {
            return;
        }
        self.writes = self.writes.saturating_add(1);
        self.pixels = self.pixels.saturating_add(pixels);
    }

    /// Lets exactly the next present through even with no writes behind it.
    pub fn force_next(&mut self) {
        self.forced = true;
    }

    /// Whether this beat has earned a present, consuming the answer.
    ///
    /// WHY it consumes: one beat that drew once gets one present. A second call
    /// in the same beat answers `false`, which is what removes the double flip
    /// without needing to find and delete every redundant call site by hand.
    pub fn take_permission(&mut self) -> bool {
        let allowed = self.writes != 0 || self.forced;
        self.writes = 0;
        self.pixels = 0;
        self.forced = false;
        allowed
    }

    /// Pixels written since the beat began — the honest `px=` field.
    pub fn pixels(&self) -> usize {
        self.pixels
    }
}

impl Default for PresentGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Which view the surface is holding.
///
/// WHY a separate record instead of trusting `state.current_view`: the handlers
/// that moved `current_view` without painting are the defect. Here the painted
/// view is written *only* by the code that drew it, so asking "does this surface
/// already show the view this tap is being answered from?" has a real answer
/// rather than an assertion.
///
/// `V` is the shell's own view enum, kept generic so this module does not have to
/// own a copy of it and cannot disagree with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintedView<V: Copy + PartialEq> {
    painted: V,
}

impl<V: Copy + PartialEq> PaintedView<V> {
    /// Starts from the view the shell's very first frame paints.
    pub const fn new(painted: V) -> Self {
        PaintedView { painted }
    }

    /// The view whose bytes are in the surface right now.
    pub const fn painted(&self) -> V {
        self.painted
    }

    /// Records that a whole-surface paint of `view` is what this frame is drawing,
    /// and returns the view the painter must render.
    ///
    /// WHY a full paint is the only thing allowed to write this: it is the only
    /// operation that can put every row of a new view on the panel. A band cannot,
    /// which is exactly what [`PaintedView::band_is_safe`] is for.
    pub fn paint_full(&mut self, view: V) -> V {
        self.painted = view;
        view
    }

    /// Whether a band of `view` may be drawn over the surface as it stands.
    ///
    /// WHY `false` means "paint the whole surface instead": the rows a band would
    /// draw belong to `view`, while the rows it leaves alone belong to whatever is
    /// already there. That mixture is the frame where a tap on a painted drawer
    /// tile was answered from the home grid. Escalating costs rows; it buys one
    /// surface that matches one view.
    pub fn band_is_safe(&self, view: V) -> bool {
        self.painted == view
    }
}
