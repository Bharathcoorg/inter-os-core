//! InterOS UI Node Tree & Semantic Accessibility Primitives.
//!
//! Provides `#![no_std]`, zero-heap-allocation UI nodes and retained tree data
//! structures for layout and accessibility traversal independently from raster pixels.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UiSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UiPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UiInsets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UiBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl UiBounds {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub const fn inset(self, p: UiInsets) -> Self {
        let w = self.width.saturating_sub(p.left.saturating_add(p.right));
        let h = self.height.saturating_sub(p.top.saturating_add(p.bottom));
        Self {
            x: self.x.saturating_add(p.left as i32),
            y: self.y.saturating_add(p.top as i32),
            width: w,
            height: h,
        }
    }

    pub const fn hit_test(&self, p: UiPoint) -> bool {
        p.x >= self.x
            && p.y >= self.y
            && p.x < self.x.saturating_add(self.width as i32)
            && p.y < self.y.saturating_add(self.height as i32)
    }
}

/// Semantic roles for UI controls and accessibility nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UiRole {
    #[default]
    Generic,
    Button,
    Checkbox,
    TextField,
    Image,
    List,
    Dialog,
    Navigation,
    Heading,
    StaticText,
    Switch,
}

impl UiRole {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Button => "button",
            Self::Checkbox => "checkbox",
            Self::TextField => "text field",
            Self::Image => "image",
            Self::List => "list",
            Self::Dialog => "dialog",
            Self::Navigation => "navigation",
            Self::Heading => "heading",
            Self::StaticText => "text",
            Self::Switch => "switch",
        }
    }

    pub const fn is_interactive(&self) -> bool {
        matches!(
            self,
            Self::Button | Self::Checkbox | Self::TextField | Self::Navigation | Self::Switch
        )
    }
}

/// A retained UI node with geometry, state, and accessibility metadata.
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
    pub const fn new(id: u32, role: UiRole, bounds: UiBounds) -> Self {
        Self {
            id,
            role,
            bounds,
            enabled: true,
            focused: false,
            accessibility_label: "",
        }
    }

    pub const fn with_label(
        id: u32,
        role: UiRole,
        bounds: UiBounds,
        accessibility_label: &'static str,
    ) -> Self {
        Self {
            id,
            role,
            bounds,
            enabled: true,
            focused: false,
            accessibility_label,
        }
    }

    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub const fn with_focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub const fn with_accessibility_label(mut self, label: &'static str) -> Self {
        self.accessibility_label = label;
        self
    }

    pub const fn hit_test(&self, p: UiPoint) -> bool {
        self.bounds.hit_test(p)
    }

    pub const fn has_accessibility_label(&self) -> bool {
        !self.accessibility_label.is_empty()
    }
}

pub const MAX_UI_NODES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct UiNodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiTreeNode {
    pub id: UiNodeId,
    pub parent: Option<UiNodeId>,
    pub node: UiNode,
}

/// Bounded retained-mode UI tree used by native applications.
/// Zero heap allocations: backed by fixed-size array storage.
pub struct UiTree {
    nodes: [Option<UiTreeNode>; MAX_UI_NODES],
    next_id: u32,
}

impl UiTree {
    pub const fn new() -> Self {
        Self {
            nodes: [None; MAX_UI_NODES],
            next_id: 1,
        }
    }

    pub fn insert(&mut self, parent: Option<UiNodeId>, mut node: UiNode) -> Option<UiNodeId> {
        if parent.is_some() && self.get(parent.unwrap()).is_none() {
            return None;
        }
        let slot = self.nodes.iter().position(|n| n.is_none())?;
        let id = UiNodeId(self.next_id.max(1));
        self.next_id = self.next_id.wrapping_add(1).max(1);
        node.id = id.0;
        self.nodes[slot] = Some(UiTreeNode { id, parent, node });
        Some(id)
    }

    pub fn get(&self, id: UiNodeId) -> Option<&UiTreeNode> {
        self.nodes.iter().flatten().find(|n| n.id == id)
    }

    pub fn node(&self, id: UiNodeId) -> Option<&UiNode> {
        self.get(id).map(|t| &t.node)
    }

    pub fn node_mut(&mut self, id: UiNodeId) -> Option<&mut UiNode> {
        self.nodes
            .iter_mut()
            .flatten()
            .find(|n| n.id == id)
            .map(|t| &mut t.node)
    }

    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn hit_test(&self, point: UiPoint) -> Option<UiNodeId> {
        self.nodes
            .iter()
            .flatten()
            .filter(|n| n.node.enabled && n.node.hit_test(point))
            .last()
            .map(|n| n.id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &UiTreeNode> {
        self.nodes.iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounds_inset_and_hit_test() {
        let b = UiBounds::new(10, 20, 100, 50);
        assert!(b.hit_test(UiPoint { x: 10, y: 20 }));
        assert!(b.hit_test(UiPoint { x: 109, y: 69 }));
        assert!(!b.hit_test(UiPoint { x: 110, y: 70 }));
        assert!(!b.hit_test(UiPoint { x: 9, y: 20 }));

        let inset_b = b.inset(UiInsets {
            left: 5,
            top: 5,
            right: 5,
            bottom: 5,
        });
        assert_eq!(inset_b.x, 15);
        assert_eq!(inset_b.y, 25);
        assert_eq!(inset_b.width, 90);
        assert_eq!(inset_b.height, 40);
    }

    #[test]
    fn test_ui_node_builder_and_roles() {
        let node = UiNode::new(1, UiRole::Button, UiBounds::new(0, 0, 48, 48))
            .with_accessibility_label("Settings")
            .with_focused(true);

        assert_eq!(node.id, 1);
        assert_eq!(node.role, UiRole::Button);
        assert!(node.role.is_interactive());
        assert_eq!(node.accessibility_label, "Settings");
        assert!(node.has_accessibility_label());
        assert!(node.focused);
        assert!(node.enabled);
    }

    #[test]
    fn test_ui_tree_insert_and_query() {
        let mut tree = UiTree::new();
        assert_eq!(tree.len(), 0);
        assert!(tree.is_empty());

        let root_node = UiNode::with_label(0, UiRole::Dialog, UiBounds::new(0, 0, 200, 200), "Modal");
        let root_id = tree.insert(None, root_node).expect("root insert failed");
        assert_eq!(tree.len(), 1);

        let btn_node = UiNode::with_label(0, UiRole::Button, UiBounds::new(10, 10, 80, 40), "Confirm");
        let btn_id = tree.insert(Some(root_id), btn_node).expect("child insert failed");
        assert_eq!(tree.len(), 2);

        let btn = tree.node(btn_id).expect("node found");
        assert_eq!(btn.accessibility_label, "Confirm");

        // Hit testing
        let hit = tree.hit_test(UiPoint { x: 15, y: 15 });
        assert_eq!(hit, Some(btn_id));

        let hit_root_only = tree.hit_test(UiPoint { x: 150, y: 150 });
        assert_eq!(hit_root_only, Some(root_id));

        let miss = tree.hit_test(UiPoint { x: 250, y: 250 });
        assert_eq!(miss, None);
    }
}
