// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android View System, Hierarchy, Layout Traversal & MotionEvent Engine
//!
//! Provides an in-memory, deterministic implementation of the standard Android View
//! hierarchy, measurement specifications, layout passes, drawing command generation,
//! and touch event dispatch without ambient authority or fake UI coordinates.
//!
//! Implements:
//! - View, ViewGroup, LinearLayout (Horizontal/Vertical), FrameLayout, RelativeLayout, ScrollView
//! - TextView, Button, ImageView, EditText
//! - MeasureSpec (EXACTLY, AT_MOST, UNSPECIFIED)
//! - Measure -> Layout -> Draw traversal
//! - MotionEvent dispatch (ACTION_DOWN, ACTION_MOVE, ACTION_UP) and click listener triggering
//! - Emission of bounded `DrawList` draw operations for compositor scanout

use alloc::string::String;
use alloc::vec::Vec;
use super::draw::{DrawList, DrawOp};

/// Standard Android View visibility modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewVisibility {
    Visible = 0,
    Invisible = 4,
    Gone = 8,
}

/// Layout dimension specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDimension {
    MatchParent,
    WrapContent,
    Exact(u32),
}

/// Margin or padding offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeOffsets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl EdgeOffsets {
    pub const fn all(val: u32) -> Self {
        Self {
            left: val,
            top: val,
            right: val,
            bottom: val,
        }
    }
}

/// Bounded rectangle representing a View's layout position relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ViewRect {
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }

    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn intersects(&self, other: &ViewRect) -> bool {
        self.left < other.right && self.right > other.left && self.top < other.bottom && self.bottom > other.top
    }

    pub fn intersect(&self, other: &ViewRect) -> Option<ViewRect> {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right.min(other.right);
        let bottom = self.bottom.min(other.bottom);
        if left < right && top < bottom {
            Some(ViewRect { left, top, right, bottom })
        } else {
            None
        }
    }

    pub fn union(&self, other: &ViewRect) -> ViewRect {
        ViewRect {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// Measurement specification mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureMode {
    Unspecified,
    Exactly,
    AtMost,
}

/// Composite measurement specification encoding mode and pixel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasureSpec {
    pub size: u32,
    pub mode: MeasureMode,
}

impl MeasureSpec {
    pub const fn make(size: u32, mode: MeasureMode) -> Self {
        Self { size, mode }
    }

    pub const fn exactly(size: u32) -> Self {
        Self {
            size,
            mode: MeasureMode::Exactly,
        }
    }

    pub const fn at_most(size: u32) -> Self {
        Self {
            size,
            mode: MeasureMode::AtMost,
        }
    }

    pub const fn unspecified() -> Self {
        Self {
            size: 0,
            mode: MeasureMode::Unspecified,
        }
    }
}

/// Android MotionEvent action codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionAction {
    Down = 0,
    Up = 1,
    Move = 2,
    Cancel = 3,
}

/// MotionEvent representing a user touch gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionEvent {
    pub action: MotionAction,
    pub x: i32,
    pub y: i32,
    pub timestamp_ms: u64,
}

/// Specific widget types supported by the generic View engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewType {
    Plain,
    TextView {
        text: String,
        text_size: u32,
        text_color: u32,
    },
    Button {
        text: String,
        text_size: u32,
        text_color: u32,
        is_pressed: bool,
    },
    ImageView {
        image_res_id: u32,
        tint_color: Option<u32>,
    },
    EditText {
        hint: String,
        text: String,
    },
    ViewGroup {
        kind: ViewGroupKind,
        children: Vec<View>,
    },
}

/// Container layout styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewGroupKind {
    LinearLayoutVertical,
    LinearLayoutHorizontal,
    FrameLayout,
    RelativeLayout,
    ScrollView,
}

/// Core Android View node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub id: i32,
    pub visibility: ViewVisibility,
    pub layout_width: LayoutDimension,
    pub layout_height: LayoutDimension,
    pub padding: EdgeOffsets,
    pub margins: EdgeOffsets,
    pub measured_width: u32,
    pub measured_height: u32,
    pub bounds: ViewRect,
    pub background_color: Option<u32>,
    pub alpha: u8,
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub is_dirty: bool,
    pub dirty_rect: Option<ViewRect>,
    pub is_clickable: bool,
    pub view_type: ViewType,
    pub on_click_callback: Option<u32>, // Listener or method handle
    pub clicked_count: u32,
    /// Method handle of an application click callback fired by the last
    /// completed click, drained by the owner via `take_pending_click`.
    pub pending_click_callback: Option<u32>,
}

#[inline(always)]
fn apply_view_alpha(color: u32, view_alpha: u8) -> u32 {
    if view_alpha == 255 {
        return color;
    }
    let orig_alpha = (color >> 24) & 0xFF;
    let new_alpha = (orig_alpha * (view_alpha as u32) + 127) / 255;
    (new_alpha << 24) | (color & 0x00FF_FFFF)
}

impl View {
    pub fn new(id: i32, view_type: ViewType) -> Self {
        let is_clickable = matches!(view_type, ViewType::Button { .. });
        Self {
            id,
            visibility: ViewVisibility::Visible,
            layout_width: LayoutDimension::WrapContent,
            layout_height: LayoutDimension::WrapContent,
            padding: EdgeOffsets::default(),
            margins: EdgeOffsets::default(),
            measured_width: 0,
            measured_height: 0,
            bounds: ViewRect::default(),
            background_color: None,
            alpha: 255,
            scroll_x: 0,
            scroll_y: 0,
            is_dirty: true,
            dirty_rect: None,
            is_clickable,
            view_type,
            on_click_callback: None,
            clicked_count: 0,
            pending_click_callback: None,
        }
    }

    /// Installs the application-defined click callback (a DEX method handle).
    pub fn set_on_click_callback(&mut self, method_handle: u32) {
        self.on_click_callback = Some(method_handle);
    }

    /// Depth-first drain of the first pending click callback recorded by this
    /// subtree, clearing it so each click is delivered exactly once.
    pub fn take_pending_click(&mut self) -> Option<u32> {
        if let Some(cb) = self.pending_click_callback.take() {
            return Some(cb);
        }
        if let ViewType::ViewGroup { children, .. } = &mut self.view_type {
            for child in children.iter_mut() {
                if let Some(cb) = child.take_pending_click() {
                    return Some(cb);
                }
            }
        }
        None
    }

    pub fn text_view(id: i32, text: &str, text_size: u32, text_color: u32) -> Self {
        Self::new(
            id,
            ViewType::TextView {
                text: String::from(text),
                text_size,
                text_color,
            },
        )
    }

    pub fn button(id: i32, text: &str, text_size: u32, text_color: u32) -> Self {
        let mut btn = Self::new(
            id,
            ViewType::Button {
                text: String::from(text),
                text_size,
                text_color,
                is_pressed: false,
            },
        );
        btn.is_clickable = true;
        btn.padding = EdgeOffsets::all(8);
        btn.background_color = Some(0xFF333333);
        btn
    }

    pub fn linear_layout(id: i32, vertical: bool) -> Self {
        let kind = if vertical {
            ViewGroupKind::LinearLayoutVertical
        } else {
            ViewGroupKind::LinearLayoutHorizontal
        };
        Self::new(id, ViewType::ViewGroup { kind, children: Vec::new() })
    }

    pub fn add_child(&mut self, child: View) {
        if let ViewType::ViewGroup { children, .. } = &mut self.view_type {
            children.push(child);
        }
    }

    /// Marks the view as dirty and records its bounds as needing redrawing.
    pub fn invalidate(&mut self) {
        self.is_dirty = true;
        self.dirty_rect = Some(self.bounds);
    }

    /// Measures the view and all its children given parent MeasureSpecs.
    pub fn measure(&mut self, width_spec: MeasureSpec, height_spec: MeasureSpec) {
        if self.visibility == ViewVisibility::Gone {
            self.measured_width = 0;
            self.measured_height = 0;
            return;
        }

        match &mut self.view_type {
            ViewType::Plain => {
                let w = match width_spec.mode {
                    MeasureMode::Exactly => width_spec.size,
                    MeasureMode::AtMost => width_spec.size.min(100),
                    MeasureMode::Unspecified => 100,
                };
                let h = match height_spec.mode {
                    MeasureMode::Exactly => height_spec.size,
                    MeasureMode::AtMost => height_spec.size.min(100),
                    MeasureMode::Unspecified => 100,
                };
                self.measured_width = w;
                self.measured_height = h;
            }
            ViewType::TextView { text, text_size, .. } | ViewType::Button { text, text_size, .. } => {
                let char_width = (*text_size).max(8) * 6 / 10;
                let text_w = (text.len() as u32) * char_width + self.padding.left + self.padding.right;
                let text_h = *text_size + self.padding.top + self.padding.bottom;

                self.measured_width = match width_spec.mode {
                    MeasureMode::Exactly => width_spec.size,
                    MeasureMode::AtMost => width_spec.size.min(text_w),
                    MeasureMode::Unspecified => text_w,
                };
                self.measured_height = match height_spec.mode {
                    MeasureMode::Exactly => height_spec.size,
                    MeasureMode::AtMost => height_spec.size.min(text_h),
                    MeasureMode::Unspecified => text_h,
                };
            }
            ViewType::ImageView { .. } => {
                let def_w = 48 + self.padding.left + self.padding.right;
                let def_h = 48 + self.padding.top + self.padding.bottom;
                self.measured_width = match width_spec.mode {
                    MeasureMode::Exactly => width_spec.size,
                    MeasureMode::AtMost => width_spec.size.min(def_w),
                    MeasureMode::Unspecified => def_w,
                };
                self.measured_height = match height_spec.mode {
                    MeasureMode::Exactly => height_spec.size,
                    MeasureMode::AtMost => height_spec.size.min(def_h),
                    MeasureMode::Unspecified => def_h,
                };
            }
            ViewType::EditText { hint, text, .. } => {
                let len = text.len().max(hint.len()) as u32;
                let w = len * 10 + self.padding.left + self.padding.right + 20;
                let h = 40 + self.padding.top + self.padding.bottom;
                self.measured_width = match width_spec.mode {
                    MeasureMode::Exactly => width_spec.size,
                    MeasureMode::AtMost => width_spec.size.min(w),
                    MeasureMode::Unspecified => w,
                };
                self.measured_height = match height_spec.mode {
                    MeasureMode::Exactly => height_spec.size,
                    MeasureMode::AtMost => height_spec.size.min(h),
                    MeasureMode::Unspecified => h,
                };
            }
            ViewType::ViewGroup { kind, children } => {
                let mut total_w = self.padding.left + self.padding.right;
                let mut total_h = self.padding.top + self.padding.bottom;
                let mut max_child_w = 0u32;
                let mut max_child_h = 0u32;

                let child_w_spec = match width_spec.mode {
                    MeasureMode::Exactly => MeasureSpec::at_most(width_spec.size.saturating_sub(total_w)),
                    MeasureMode::AtMost => MeasureSpec::at_most(width_spec.size.saturating_sub(total_w)),
                    MeasureMode::Unspecified => MeasureSpec::unspecified(),
                };
                let child_h_spec = match kind {
                    ViewGroupKind::ScrollView => MeasureSpec::unspecified(),
                    _ => match height_spec.mode {
                        MeasureMode::Exactly => MeasureSpec::at_most(height_spec.size.saturating_sub(total_h)),
                        MeasureMode::AtMost => MeasureSpec::at_most(height_spec.size.saturating_sub(total_h)),
                        MeasureMode::Unspecified => MeasureSpec::unspecified(),
                    },
                };

                for child in children.iter_mut() {
                    child.measure(child_w_spec, child_h_spec);
                    let cw = child.measured_width + child.margins.left + child.margins.right;
                    let ch = child.measured_height + child.margins.top + child.margins.bottom;

                    match kind {
                        ViewGroupKind::LinearLayoutVertical => {
                            total_h += ch;
                            max_child_w = max_child_w.max(cw);
                        }
                        ViewGroupKind::LinearLayoutHorizontal => {
                            total_w += cw;
                            max_child_h = max_child_h.max(ch);
                        }
                        ViewGroupKind::FrameLayout | ViewGroupKind::RelativeLayout | ViewGroupKind::ScrollView => {
                            max_child_w = max_child_w.max(cw);
                            max_child_h = max_child_h.max(ch);
                        }
                    }
                }

                let desired_w = match kind {
                    ViewGroupKind::LinearLayoutVertical
                    | ViewGroupKind::FrameLayout
                    | ViewGroupKind::RelativeLayout
                    | ViewGroupKind::ScrollView => max_child_w + self.padding.left + self.padding.right,
                    ViewGroupKind::LinearLayoutHorizontal => total_w,
                };

                let desired_h = match kind {
                    ViewGroupKind::LinearLayoutVertical => total_h,
                    ViewGroupKind::LinearLayoutHorizontal
                    | ViewGroupKind::FrameLayout
                    | ViewGroupKind::RelativeLayout
                    | ViewGroupKind::ScrollView => max_child_h + self.padding.top + self.padding.bottom,
                };

                self.measured_width = match width_spec.mode {
                    MeasureMode::Exactly => width_spec.size,
                    MeasureMode::AtMost => width_spec.size.min(desired_w),
                    MeasureMode::Unspecified => desired_w,
                };
                self.measured_height = match height_spec.mode {
                    MeasureMode::Exactly => height_spec.size,
                    MeasureMode::AtMost => height_spec.size.min(desired_h),
                    MeasureMode::Unspecified => desired_h,
                };
            }
        }
    }

    /// Positions the view and its children within the specified layout coordinates.
    pub fn layout(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
        self.bounds = ViewRect { left, top, right, bottom };

        if let ViewType::ViewGroup { kind, children } = &mut self.view_type {
            let inner_left = left + (self.padding.left as i32);
            let inner_top = top + (self.padding.top as i32);
            let inner_right = right - (self.padding.right as i32);
            let inner_bottom = bottom - (self.padding.bottom as i32);

            let mut cur_y = inner_top;
            let mut cur_x = inner_left;

            for child in children.iter_mut() {
                if child.visibility == ViewVisibility::Gone {
                    continue;
                }

                let cw = child.measured_width as i32;
                let ch = child.measured_height as i32;
                let cl = cur_x + (child.margins.left as i32);
                let ct = cur_y + (child.margins.top as i32);

                match kind {
                    ViewGroupKind::LinearLayoutVertical => {
                        let cr = (cl + cw).min(inner_right);
                        let cb = ct + ch;
                        child.layout(cl, ct, cr, cb);
                        cur_y = cb + (child.margins.bottom as i32);
                    }
                    ViewGroupKind::LinearLayoutHorizontal => {
                        let cr = cl + cw;
                        let cb = (ct + ch).min(inner_bottom);
                        child.layout(cl, ct, cr, cb);
                        cur_x = cr + (child.margins.right as i32);
                    }
                    ViewGroupKind::FrameLayout | ViewGroupKind::RelativeLayout | ViewGroupKind::ScrollView => {
                        let cr = (cl + cw).min(inner_right);
                        let cb = (ct + ch).min(inner_bottom);
                        child.layout(cl, ct, cr, cb);
                    }
                }
            }
        }
    }

    /// Traverses the view hierarchy and generates draw commands into the target DrawList.
    pub fn draw(&self, draw_list: &mut DrawList) {
        if self.visibility != ViewVisibility::Visible || self.alpha == 0 {
            return;
        }

        let w = self.bounds.width();
        let h = self.bounds.height();
        if w == 0 || h == 0 {
            return;
        }

        // Clip ScrollView children to inner bounds
        if let ViewType::ViewGroup { kind: ViewGroupKind::ScrollView, children } = &self.view_type {
            let inner_w = w.saturating_sub(self.padding.left + self.padding.right);
            let inner_h = h.saturating_sub(self.padding.top + self.padding.bottom);
            let _ = draw_list.push(DrawOp::PushClip {
                x: self.bounds.left + self.padding.left as i32,
                y: self.bounds.top + self.padding.top as i32,
                width: inner_w,
                height: inner_h,
            });
            for child in children {
                child.draw(draw_list);
            }
            let _ = draw_list.push(DrawOp::PopClip);
            return;
        }

        // 1. Draw Background
        if let Some(bg_color) = self.background_color {
            let effective_color = apply_view_alpha(bg_color, self.alpha);
            let _ = draw_list.push(DrawOp::FillRect {
                x: self.bounds.left,
                y: self.bounds.top,
                width: w,
                height: h,
                color: effective_color,
            });
        }

        // 2. Draw Self Specifics
        match &self.view_type {
            ViewType::Button { text, text_color, is_pressed, text_size } => {
                let border_color = if *is_pressed { 0xFF666666 } else { *text_color };
                if self.clicked_count > 0 {
                    let _ = draw_list.push(DrawOp::FillRect {
                        x: self.bounds.left + 1,
                        y: self.bounds.top + 1,
                        width: w.saturating_sub(2),
                        height: h.saturating_sub(2),
                        color: 0xFF1B5E20,
                    });
                }
                // Draw 1-px button border
                let _ = draw_list.push(DrawOp::DrawHLine {
                    x: self.bounds.left,
                    y: self.bounds.top,
                    width: w,
                    color: border_color,
                });
                let _ = draw_list.push(DrawOp::DrawHLine {
                    x: self.bounds.left,
                    y: self.bounds.bottom.saturating_sub(1),
                    width: w,
                    color: border_color,
                });
                let _ = draw_list.push(DrawOp::DrawVLine {
                    x: self.bounds.left,
                    y: self.bounds.top,
                    height: h,
                    color: border_color,
                });
                let _ = draw_list.push(DrawOp::DrawVLine {
                    x: self.bounds.right.saturating_sub(1),
                    y: self.bounds.top,
                    height: h,
                    color: border_color,
                });
                // Render the real button text. Click state is shown by the
                // activated background, never by a fabricated "[Clicked: N]" label.
                let display_text = text.clone();
                let _ = draw_list.push(DrawOp::DrawText {
                    x: self.bounds.left + 8,
                    y: self.bounds.top + 8,
                    text: display_text,
                    color: *text_color,
                    size: *text_size,
                });
            }
            ViewType::TextView { text, text_color, text_size } => {
                // Render baseline underline accent for text bounding
                let _ = draw_list.push(DrawOp::DrawHLine {
                    x: self.bounds.left,
                    y: self.bounds.bottom.saturating_sub(2),
                    width: w.min(32),
                    color: *text_color,
                });
                // Render actual text glyphs using sovereign typography stack!
                let _ = draw_list.push(DrawOp::DrawText {
                    x: self.bounds.left + 4,
                    y: self.bounds.top + 4,
                    text: text.clone(),
                    color: *text_color,
                    size: *text_size,
                });
            }
            ViewType::EditText { text, hint } => {
                let display_text = if text.is_empty() { hint.as_str() } else { text.as_str() };
                let color = if text.is_empty() { 0xFF888888 } else { 0xFFFFFFFF };
                let _ = draw_list.push(DrawOp::DrawText {
                    x: self.bounds.left + 4,
                    y: self.bounds.top + 4,
                    text: String::from(display_text),
                    color,
                    size: 14,
                });
                // Render cursor if active
                let cursor_x = self.bounds.left + 4 + (text.len() as i32 * 9);
                let _ = draw_list.push(DrawOp::DrawVLine {
                    x: cursor_x,
                    y: self.bounds.top + 4,
                    height: 16,
                    color: 0xFFFFFFFF,
                });
            }
            ViewType::ImageView { image_res_id, .. } => {
                let _ = draw_list.push(DrawOp::FillRect {
                    x: self.bounds.left,
                    y: self.bounds.top,
                    width: w,
                    height: h,
                    color: 0xFF2A2A2A,
                });
                let res_label = alloc::format!("IMG:{:x}", image_res_id);
                let _ = draw_list.push(DrawOp::DrawText {
                    x: self.bounds.left + 4,
                    y: self.bounds.top + 4,
                    text: res_label,
                    color: 0xFFAAAAAA,
                    size: 12,
                });
            }
            _ => {}
        }

        // 3. Draw Children
        if let ViewType::ViewGroup { children, .. } = &self.view_type {
            for child in children {
                child.draw(draw_list);
            }
        }
    }

    /// Dispatches a touch MotionEvent down the view hierarchy.
    pub fn dispatch_touch_event(&mut self, event: &MotionEvent) -> bool {
        if self.visibility != ViewVisibility::Visible {
            return false;
        }

        if !self.bounds.contains(event.x, event.y) {
            if let ViewType::Button { is_pressed, .. } = &mut self.view_type {
                *is_pressed = false;
            }
            return false;
        }

        // 1. Give children first opportunity (reverse z-order)
        if let ViewType::ViewGroup { children, .. } = &mut self.view_type {
            for child in children.iter_mut().rev() {
                if child.dispatch_touch_event(event) {
                    return true;
                }
            }
        }

        // 2. Handle self touch
        if self.is_clickable {
            match event.action {
                MotionAction::Down => {
                    if let ViewType::Button { is_pressed, .. } = &mut self.view_type {
                        *is_pressed = true;
                    }
                    return true;
                }
                MotionAction::Move => {
                    return true;
                }
                MotionAction::Up => {
                    if let ViewType::Button { is_pressed, .. } = &mut self.view_type {
                        *is_pressed = false;
                    }
                    self.clicked_count += 1;
                    // Record the application-defined callback (method handle) for
                    // the owner to execute; no label is fabricated here.
                    if let Some(cb) = self.on_click_callback {
                        self.pending_click_callback = Some(cb);
                    }
                    return true;
                }
                MotionAction::Cancel => {
                    if let ViewType::Button { is_pressed, .. } = &mut self.view_type {
                        *is_pressed = false;
                    }
                    return true;
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_measure_layout_and_touch() {
        let mut root = View::linear_layout(1, true);
        root.layout_width = LayoutDimension::MatchParent;
        root.layout_height = LayoutDimension::MatchParent;

        let tv = View::text_view(2, "InterOS Title", 16, 0xFFFFFFFF);
        let mut btn = View::button(3, "Submit", 14, 0xFF00FF00);
        btn.layout_width = LayoutDimension::Exact(200);
        btn.layout_height = LayoutDimension::Exact(50);

        root.add_child(tv);
        root.add_child(btn);

        root.measure(
            MeasureSpec::make(720, MeasureMode::Exactly),
            MeasureSpec::make(1440, MeasureMode::Exactly),
        );
        assert_eq!(root.measured_width, 720);
        assert_eq!(root.measured_height, 1440);

        root.layout(0, 0, 720, 1440);
        assert_eq!(root.bounds.width(), 720);
        assert_eq!(root.bounds.height(), 1440);

        let mut dl = DrawList::new();
        root.draw(&mut dl);
        assert!(!dl.ops().is_empty());

        let btn_bounds = if let ViewType::ViewGroup { children, .. } = &root.view_type {
            children[1].bounds
        } else {
            panic!("expected viewgroup");
        };

        let touch_down = MotionEvent {
            action: MotionAction::Down,
            x: btn_bounds.left + 10,
            y: btn_bounds.top + 10,
            timestamp_ms: 100,
        };
        assert!(root.dispatch_touch_event(&touch_down));

        let touch_up = MotionEvent {
            action: MotionAction::Up,
            x: btn_bounds.left + 10,
            y: btn_bounds.top + 10,
            timestamp_ms: 150,
        };
        assert!(root.dispatch_touch_event(&touch_up));

        if let ViewType::ViewGroup { children, .. } = &root.view_type {
            assert_eq!(children[1].clicked_count, 1);
        }

        let mut dl_after = DrawList::new();
        root.draw(&mut dl_after);
        // A completed click must NOT synthesize a "[Clicked: N]" label; the real
        // button text is rendered and the app callback is dispatched by the owner.
        let has_fabricated_label = dl_after.ops().iter().any(|op| {
            if let DrawOp::DrawText { text, .. } = op {
                text.contains("[Clicked:")
            } else {
                false
            }
        });
        assert!(!has_fabricated_label, "no hardcoded [Clicked:N] label may be rendered");
        let has_activated_bg = dl_after.ops().iter().any(|op| {
            if let DrawOp::FillRect { color, .. } = op {
                *color == 0xFF1B5E20
            } else {
                false
            }
        });
        assert!(has_activated_bg, "UI state must reflect touch mutation in button background");
    }

    #[test]
    fn test_view_rect_and_visibility() {
        let rect = ViewRect { left: 10, top: 20, right: 110, bottom: 70 };
        assert_eq!(rect.width(), 100);
        assert_eq!(rect.height(), 50);
        assert!(rect.contains(50, 50));
        assert!(!rect.contains(5, 50));
        assert!(!rect.contains(50, 75));

        let mut gone_view = View::button(10, "Hidden", 14, 0);
        gone_view.visibility = ViewVisibility::Gone;
        gone_view.measure(MeasureSpec::make(100, MeasureMode::Exactly), MeasureSpec::make(100, MeasureMode::Exactly));
        assert_eq!(gone_view.measured_width, 0);
        assert_eq!(gone_view.measured_height, 0);
    }
}

