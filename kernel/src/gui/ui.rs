//! Reusable InterOS-native UI primitives.
//!
//! This layer describes layout and interaction semantics independently from
//! framebuffer pixels. Applications can use the same primitives across
//! orientations and display sizes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiSize { pub width: u32, pub height: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiPoint { pub x: i32, pub y: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiInsets { pub left: u32, pub top: u32, pub right: u32, pub bottom: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis { Horizontal, Vertical }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment { Start, Center, End, Stretch }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiBounds { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }

impl UiBounds {
    pub const fn inset(self, p: UiInsets) -> Self {
        let w = self.width.saturating_sub(p.left.saturating_add(p.right));
        let h = self.height.saturating_sub(p.top.saturating_add(p.bottom));
        Self { x: self.x.saturating_add(p.left as i32), y: self.y.saturating_add(p.top as i32), width: w, height: h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiRole { Generic, Button, Checkbox, TextField, Image, List, Dialog, Navigation }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiNode {
    pub id: u32,
    pub role: UiRole,
    pub bounds: UiBounds,
    pub enabled: bool,
    pub focused: bool,
    pub accessibility_label: &'static str,
}

impl UiNode {
    pub const fn hit_test(&self, p: UiPoint) -> bool {
        p.x >= self.bounds.x && p.y >= self.bounds.y &&
            p.x < self.bounds.x.saturating_add(self.bounds.width as i32) &&
            p.y < self.bounds.y.saturating_add(self.bounds.height as i32)
    }
}

pub struct UiLayout {
    pub axis: Axis,
    pub alignment: Alignment,
    pub spacing: u32,
    pub padding: UiInsets,
}

impl UiLayout {
    pub const fn new(axis: Axis) -> Self {
        Self { axis, alignment: Alignment::Start, spacing: 0, padding: UiInsets { left:0, top:0, right:0, bottom:0 } }
    }

    pub fn place(&self, parent: UiBounds, count: usize, child_main: u32) -> (UiBounds, usize) {
        if count == 0 { return (parent.inset(self.padding), 0); }
        let area = parent.inset(self.padding);
        let total_spacing = self.spacing.saturating_mul(count.saturating_sub(1) as u32);
        let available = match self.axis { Axis::Horizontal => area.width, Axis::Vertical => area.height };
        let child = child_main.min(available.saturating_sub(total_spacing));
        (area, child as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiAnimation {
    pub start_ms: u64,
    pub duration_ms: u32,
    pub from: i32,
    pub to: i32,
}

impl UiAnimation {
    pub const fn new(start_ms: u64, duration_ms: u32, from: i32, to: i32) -> Self {
        Self { start_ms, duration_ms, from, to }
    }

    pub fn value_at(&self, now_ms: u64) -> i32 {
        if now_ms <= self.start_ms { return self.from; }
        let end_ms = self.start_ms.saturating_add(self.duration_ms as u64);
        if self.duration_ms == 0 || now_ms >= end_ms { return self.to; }
        let elapsed = now_ms - self.start_ms;
        self.from + (((self.to - self.from) as i64 * elapsed as i64) / self.duration_ms as i64) as i32
    }
}


/// Bounded retained-mode UI tree used by native applications.
/// Nodes own stable IDs and parent/child relationships; rendering remains a
/// separate concern so layout and accessibility do not depend on pixels.
pub const MAX_UI_NODES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiNodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiTreeNode {
    pub id: UiNodeId,
    pub parent: Option<UiNodeId>,
    pub node: UiNode,
}

pub struct UiTree {
    nodes: [Option<UiTreeNode>; MAX_UI_NODES],
    next_id: u32,
}

impl UiTree {
    pub const fn new() -> Self { Self { nodes: [None; MAX_UI_NODES], next_id: 1 } }

    pub fn insert(&mut self, parent: Option<UiNodeId>, mut node: UiNode) -> Option<UiNodeId> {
        if parent.is_some() && self.get(parent.unwrap()).is_none() { return None; }
        let slot = self.nodes.iter().position(|n| n.is_none())?;
        let id = UiNodeId(self.next_id.max(1));
        self.next_id = self.next_id.wrapping_add(1).max(1);
        node.id = id.0;
        self.nodes[slot] = Some(UiTreeNode { id, parent, node });
        Some(id)
    }

    pub fn get(&self, id: UiNodeId) -> Option<&UiTreeNode> { self.nodes.iter().flatten().find(|n| n.id == id) }
    pub fn node(&self, id: UiNodeId) -> Option<&UiTreeNode> { self.nodes.iter().flatten().find(|n| n.id == id) }
    pub fn node_mut(&mut self, id: UiNodeId) -> Option<&mut UiTreeNode> { self.nodes.iter_mut().flatten().find(|n| n.id == id) }
    pub fn hit_test(&self, point: UiPoint) -> Option<UiNodeId> {
        self.nodes.iter().flatten().filter(|n| n.node.enabled && n.node.hit_test(point)).last().map(|n| n.id)
    }
    pub fn len(&self) -> usize { self.nodes.iter().filter(|n| n.is_some()).count() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}
