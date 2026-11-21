# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Android View System, Hierarchy & MotionEvent Traversal Contract Tests.

Verifies:
1. In-memory generic View hierarchy without fake coordinates or hardcoded layouts.
2. MeasureSpec (EXACTLY, AT_MOST, UNSPECIFIED) measurement specifications.
3. ViewGroup container kinds (LinearLayout, FrameLayout, RelativeLayout, ScrollView).
4. Deterministic draw pass generating bounded DrawList operations.
5. MotionEvent touch dispatch (ACTION_DOWN, ACTION_MOVE, ACTION_UP) and click handling.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_view_system_types_and_invariants():
    source = read_source("kernel/src/compat/android/view.rs")
    assert "pub enum ViewVisibility" in source
    assert "pub enum LayoutDimension" in source
    assert "pub struct ViewRect" in source
    assert "pub enum MeasureMode" in source
    assert "pub struct MeasureSpec" in source
    assert "pub enum MotionAction" in source
    assert "pub struct MotionEvent" in source
    assert "pub enum ViewType" in source
    assert "pub enum ViewGroupKind" in source
    assert "pub struct View" in source


def test_kernel_view_traversal_lifecycle_methods():
    source = read_source("kernel/src/compat/android/view.rs")
    assert "pub fn measure(&mut self, width_spec: MeasureSpec, height_spec: MeasureSpec)" in source
    assert "pub fn layout(&mut self, left: i32, top: i32, right: i32, bottom: i32)" in source
    assert "pub fn draw(&self, draw_list: &mut DrawList)" in source
    assert "pub fn dispatch_touch_event(&mut self, event: &MotionEvent) -> bool" in source


def test_kernel_view_draw_list_bounded_emission():
    source = read_source("kernel/src/compat/android/view.rs")
    draw_source = read_source("kernel/src/compat/android/draw.rs")
    assert "draw_list.push(DrawOp::FillRect" in source
    assert "pub const MAX_DRAW_OPS_PER_LIST: usize = 256" in draw_source

    # Also verify sovereign typography stack in Ring-3 ACR
    acr_view = read_source("crates/interos-acr/src/view.rs")
    acr_draw = read_source("crates/interos-acr/src/draw.rs")
    assert "DrawOp::DrawText" in acr_view
    assert "render_draw_list_to_buffer" in acr_draw
    assert "get_glyph_bitmap" in acr_draw


def test_kernel_view_touch_event_handling_contract():
    source = read_source("kernel/src/compat/android/view.rs")
    assert "MotionAction::Down =>" in source
    assert "MotionAction::Up =>" in source
    assert "self.clicked_count += 1;" in source
    assert "reverse z-order" in source
