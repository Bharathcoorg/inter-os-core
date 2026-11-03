// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Accessibility, Dynamic Type & Screen Reader Subsystem.
//!
//! Provides `#![no_std]`, zero-heap-allocation accessibility services:
//! - Dynamic Type user text scaling (compact 80%, normal 100%, large 125%, extra large 150%)
//! - Accessibility tree extraction and focus navigation from `UiNode` hierarchies
//! - Screen reader readout formatters and announcements for non-visual interaction
//! - WCAG 2.1 AA relative luminance and contrast ratio validation (4.5:1 floor)
//!
//! The dynamic type here is not a parallel ladder. [`DynamicType::publish`]
//! hands the level to `crate::font::USER_TEXT_SCALE`, and the runtime helpers
//! beside the ladder (`cap_for_scale_user`, `line_height_user`,
//! `em_for_scale_user`, `measure_user`) draw at it -- cap height, em, line box
//! and advance all from the one published level, because a scaled ink in an
//! unscaled box is clipped text. An application that has published nothing
//! draws the ladder exactly: the default level is a hundred per cent.

use super::node::{UiBounds, UiNode, UiPoint, UiRole, UiTree};
use crate::font;

// --- Dynamic Type & User Text Scale -----------------------------------------

/// User text scale configuration for Dynamic Type accessibility scaling.
///
/// Levels correspond to:
/// - `Compact` (0): 80% of canonical type scale (floor bounded to 10px minimum)
/// - `Normal` (1): 100% of canonical type scale (default)
/// - `Large` (2): 125% of canonical type scale
/// - `ExtraLarge` (3): 150% of canonical type scale
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum UserTextScale {
    Compact = 0,
    #[default]
    Normal = 1,
    Large = 2,
    ExtraLarge = 3,
}

impl UserTextScale {
    pub const fn from_u8(level: u8) -> Self {
        match level {
            0 => Self::Compact,
            1 => Self::Normal,
            2 => Self::Large,
            _ => Self::ExtraLarge, // Clamped gracefully to maximum supported scaling
        }
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn percentage(self) -> u16 {
        match self {
            Self::Compact => 80,
            Self::Normal => 100,
            Self::Large => 125,
            Self::ExtraLarge => 150,
        }
    }

    /// Reads the level out of the value the settings store carries.
    ///
    /// WHY both spellings: `display.text_scale` is written by the platform's
    /// own Settings app as a word (`b"large"`), and a numeric level is what the
    /// `UserTextScale` discriminant is, so a store that holds either must answer
    /// the same way for every reader. An unrecognised value answers `None`,
    /// which is the caller's signal to keep its own default rather than to
    /// invent a rung the person never chose.
    ///
    /// WHY the longest form first: `extra_large`, `extralarge` and `3` all mean
    /// the ladder's largest setting, and a match that stopped at the numeric
    /// prefix would read `3` as its own answer twice over.
    pub fn from_settings_value(value: &[u8]) -> Option<Self> {
        const WORDS: [(&[u8], UserTextScale); 8] = [
            (b"compact", UserTextScale::Compact),
            (b"normal", UserTextScale::Normal),
            (b"large", UserTextScale::Large),
            (b"extralarge", UserTextScale::ExtraLarge),
            (b"extra_large", UserTextScale::ExtraLarge),
            (b"extra-large", UserTextScale::ExtraLarge),
            (b"extra large", UserTextScale::ExtraLarge),
            (b"xl", UserTextScale::ExtraLarge),
        ];
        for (word, level) in WORDS {
            if value.eq_ignore_ascii_case(word) {
                return Some(level);
            }
        }
        if value.len() == 1 {
            return match value[0] {
                b'0' => Some(UserTextScale::Compact),
                b'1' => Some(UserTextScale::Normal),
                b'2' => Some(UserTextScale::Large),
                b'3' => Some(UserTextScale::ExtraLarge),
                _ => None,
            };
        }
        None
    }
}

/// Dynamic type engine applying user text scaling to typography metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DynamicType {
    pub scale: UserTextScale,
}

impl DynamicType {
    pub const fn new(scale: UserTextScale) -> Self {
        Self { scale }
    }

    pub const fn from_level(level: u8) -> Self {
        Self {
            scale: UserTextScale::from_u8(level),
        }
    }

    /// The dynamic type the platform is drawing with right now.
    ///
    /// WHY this reads a cell and not a settings query: the store is the person's
    /// answer, and this cell is the answer the renderer was handed. They agree
    /// because an application publishes the one it read -- see
    /// [`DynamicType::publish`] -- and an application that never read the store
    /// keeps the default, which is [`UserTextScale::Normal`] and therefore the
    /// unscaled ladder.
    #[inline]
    pub fn current() -> Self {
        Self::from_level(crate::font::user_text_scale())
    }

    /// Hands this level to the renderer, and returns what was stored.
    ///
    /// This is the hop between the setting and a painted pixel: after it, the
    /// `*_user` helpers in [`crate::font`] -- [`crate::font::cap_for_scale_user`],
    /// [`crate::font::line_height_user`], [`crate::font::em_for_scale_user`] and
    /// [`crate::font::measure_user`] -- answer at this level, and every surface
    /// that re-derives its geometry from them moves its boxes with its ink.
    ///
    /// WHY the return value: a level above the ladder's largest setting is
    /// clamped, and a caller that shows the person the setting it took should
    /// show the one the platform actually drew with.
    ///
    /// WHY it is not called per glyph: publishing once where the application
    /// already polls the store keeps the settings query out of the paint loop;
    /// the render path then reads one integer.
    #[inline]
    pub fn publish(self) -> UserTextScale {
        UserTextScale::from_u8(crate::font::set_user_text_scale(self.scale.as_u8()))
    }

    /// The scaled cap height in pixels for a given type scale rung.
    #[inline]
    pub const fn cap_height(&self, scale_rung: usize) -> u32 {
        font::cap_for_user_scale(scale_rung, self.scale.as_u8())
    }

    /// The scaled em in pixels for a given type scale rung.
    #[inline]
    pub const fn em(&self, scale_rung: usize) -> u32 {
        font::em_for_user_scale(scale_rung, self.scale.as_u8())
    }

    /// The scaled line box in pixels for a given type scale rung.
    #[inline]
    pub const fn line_height(&self, scale_rung: usize) -> usize {
        font::line_height_for_user_scale(scale_rung, self.scale.as_u8())
    }

    /// The scaled ascent in pixels for a given type scale rung.
    #[inline]
    pub const fn ascent(&self, scale_rung: usize) -> usize {
        font::ascent_for_user_scale(scale_rung, self.scale.as_u8())
    }

    /// Measures text width under current user text scale.
    #[inline]
    pub fn measure(&self, text: &[u8], scale_rung: usize) -> usize {
        font::measure_user_scale(text, scale_rung, self.scale.as_u8())
    }

    /// Measures bold text width under current user text scale.
    #[inline]
    pub fn measure_bold(&self, text: &[u8], scale_rung: usize) -> usize {
        font::measure_bold_user_scale(text, scale_rung, self.scale.as_u8())
    }
}

// --- Accessibility Node & Tree Walker ---------------------------------------

/// Extracted semantic representation of a UI node for screen readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessibilityNode {
    pub id: u32,
    pub role: UiRole,
    pub bounds: UiBounds,
    pub label: &'static str,
    pub enabled: bool,
    pub focused: bool,
}

impl AccessibilityNode {
    /// Extracts an accessibility node from a `UiNode`.
    ///
    /// Non-interactive containers with empty labels are excluded to eliminate
    /// spoken noise during accessibility navigation.
    pub const fn from_ui_node(node: &UiNode) -> Option<Self> {
        if node.has_accessibility_label() || node.role.is_interactive() {
            Some(Self {
                id: node.id,
                role: node.role,
                bounds: node.bounds,
                label: node.accessibility_label,
                enabled: node.enabled,
                focused: node.focused,
            })
        } else {
            None
        }
    }

    pub const fn is_focusable(&self) -> bool {
        self.enabled && self.role.is_interactive()
    }
}

/// Maximum accessibility nodes extracted in a single bounded traversal pass.
pub const MAX_ACCESSIBILITY_NODES: usize = 64;

/// Traverses retained `UiTree` hierarchies to extract accessibility elements.
pub struct AccessibilityTreeWalker<'a> {
    tree: &'a UiTree,
}

impl<'a> AccessibilityTreeWalker<'a> {
    pub const fn new(tree: &'a UiTree) -> Self {
        Self { tree }
    }

    /// Iterates over all accessible nodes, invoking `visitor` for each.
    pub fn walk<F: FnMut(&AccessibilityNode)>(&self, mut visitor: F) {
        for tree_node in self.tree.iter() {
            if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                visitor(&acc_node);
            }
        }
    }

    /// Collects accessible nodes into a caller-supplied fixed-size buffer.
    ///
    /// Returns the number of nodes populated. Guaranteed zero heap allocations.
    pub fn collect_nodes<const N: usize>(&self, out: &mut [Option<AccessibilityNode>; N]) -> usize {
        let mut count = 0;
        for tree_node in self.tree.iter() {
            if count >= N {
                break;
            }
            if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                out[count] = Some(acc_node);
                count += 1;
            }
        }
        count
    }

    /// Finds the currently focused accessible node, if any.
    pub fn find_focused(&self) -> Option<AccessibilityNode> {
        for tree_node in self.tree.iter() {
            if tree_node.node.focused {
                if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                    return Some(acc_node);
                }
            }
        }
        None
    }

    /// Finds the next focusable accessible node after `current_id` in tree order.
    pub fn find_next_focusable(&self, current_id: Option<u32>) -> Option<AccessibilityNode> {
        let mut past_current = current_id.is_none();
        for tree_node in self.tree.iter() {
            if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                if !past_current {
                    if Some(acc_node.id) == current_id {
                        past_current = true;
                    }
                    continue;
                }
                if acc_node.is_focusable() {
                    return Some(acc_node);
                }
            }
        }
        // If we reached the end or started after current, wrap around from beginning
        if current_id.is_some() {
            for tree_node in self.tree.iter() {
                if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                    if Some(acc_node.id) == current_id {
                        break;
                    }
                    if acc_node.is_focusable() {
                        return Some(acc_node);
                    }
                }
            }
        }
        None
    }

    /// Finds the previous focusable accessible node before `current_id` in tree order.
    pub fn find_prev_focusable(&self, current_id: Option<u32>) -> Option<AccessibilityNode> {
        let mut prev_candidate = None;
        for tree_node in self.tree.iter() {
            if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                if Some(acc_node.id) == current_id && prev_candidate.is_some() {
                    return prev_candidate;
                }
                if acc_node.is_focusable() {
                    prev_candidate = Some(acc_node);
                }
            }
        }
        prev_candidate
    }

    /// Hit-tests a point to find the topmost accessible node.
    pub fn node_at_point(&self, point: UiPoint) -> Option<AccessibilityNode> {
        let mut matched = None;
        for tree_node in self.tree.iter() {
            if tree_node.node.enabled && tree_node.node.hit_test(point) {
                if let Some(acc_node) = AccessibilityNode::from_ui_node(&tree_node.node) {
                    matched = Some(acc_node);
                }
            }
        }
        matched
    }
}

// --- Screen Reader Readout Formatter ----------------------------------------

/// Formats a spoken accessibility announcement from a `UiNode`.
///
/// Converts a node's label, role, and focus/enabled state into a human-readable
/// readout string (e.g. `"Settings, button"`, `"Search, text field, focused"`).
/// Writes directly into caller's slice buffer without heap allocation.
pub struct ReadoutFormatter;

impl ReadoutFormatter {
    /// Formats a spoken accessibility string for `node` into `buf`.
    ///
    /// Returns the number of bytes written.
    pub fn format(node: &UiNode, buf: &mut [u8]) -> usize {
        let mut cursor = 0;

        let write_slice = |buf: &mut [u8], cursor: &mut usize, slice: &[u8]| {
            let available = buf.len().saturating_sub(*cursor);
            let count = available.min(slice.len());
            buf[*cursor..*cursor + count].copy_from_slice(&slice[..count]);
            *cursor += count;
        };

        if !node.accessibility_label.is_empty() {
            write_slice(buf, &mut cursor, node.accessibility_label.as_bytes());
        } else {
            write_slice(buf, &mut cursor, node.role.as_str().as_bytes());
            if !node.enabled {
                write_slice(buf, &mut cursor, b", disabled");
            }
            if node.focused {
                write_slice(buf, &mut cursor, b", focused");
            }
            return cursor;
        }

        match node.role {
            UiRole::Button => write_slice(buf, &mut cursor, b", button"),
            UiRole::Checkbox => write_slice(buf, &mut cursor, b", checkbox"),
            UiRole::TextField => write_slice(buf, &mut cursor, b", text field"),
            UiRole::Navigation => write_slice(buf, &mut cursor, b", navigation"),
            UiRole::Heading => write_slice(buf, &mut cursor, b", heading"),
            UiRole::Switch => write_slice(buf, &mut cursor, b", switch"),
            UiRole::Dialog => write_slice(buf, &mut cursor, b", dialog"),
            UiRole::List => write_slice(buf, &mut cursor, b", list"),
            UiRole::Image => write_slice(buf, &mut cursor, b", image"),
            UiRole::Generic | UiRole::StaticText => {}
        }

        if !node.enabled {
            write_slice(buf, &mut cursor, b", disabled");
        }

        if node.focused {
            write_slice(buf, &mut cursor, b", focused");
        }

        cursor
    }
}

/// Priority for accessibility screen reader announcements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnnouncementPriority {
    #[default]
    Polite,
    Assertive,
}

/// A stack-allocated screen reader announcement buffer.
#[derive(Clone, Copy)]
pub struct AccessibilityAnnouncement {
    pub priority: AnnouncementPriority,
    pub length: usize,
    pub bytes: [u8; 128],
}

impl AccessibilityAnnouncement {
    pub fn from_node(node: &UiNode, priority: AnnouncementPriority) -> Self {
        let mut bytes = [0u8; 128];
        let length = ReadoutFormatter::format(node, &mut bytes);
        Self {
            priority,
            length,
            bytes,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

// --- WCAG 2.1 Contrast Ratio Verification -----------------------------------

/// Linearized sRGB luminance lookup table (0..=255 mapped to 0..=10000).
/// Precomputed for zero-allocation `#![no_std]` contrast evaluation.
const SRGB_TO_LINEAR_X10000: [u16; 256] = [
        0,     3,     6,     9,    12,    15,    18,    21,    24,    27,    30,    33,    37,    40,    44,    48,
       52,    56,    60,    65,    70,    75,    80,    86,    91,    97,   103,   110,   116,   123,   130,   137,
      144,   152,   160,   168,   176,   185,   194,   203,   212,   222,   232,   242,   252,   262,   273,   284,
      296,   307,   319,   331,   343,   356,   369,   382,   395,   409,   423,   437,   452,   467,   482,   497,
      513,   529,   545,   561,   578,   595,   612,   630,   648,   666,   685,   704,   723,   742,   762,   782,
      802,   823,   844,   865,   887,   908,   931,   953,   976,   999,  1022,  1046,  1070,  1095,  1119,  1144,
     1170,  1195,  1221,  1248,  1274,  1301,  1329,  1356,  1384,  1413,  1441,  1470,  1500,  1529,  1559,  1590,
     1620,  1651,  1683,  1714,  1746,  1779,  1812,  1845,  1878,  1912,  1946,  1981,  2016,  2051,  2086,  2122,
     2159,  2195,  2232,  2270,  2307,  2346,  2384,  2423,  2462,  2502,  2542,  2582,  2623,  2664,  2705,  2747,
     2789,  2831,  2874,  2918,  2961,  3005,  3050,  3095,  3140,  3185,  3231,  3278,  3325,  3372,  3419,  3467,
     3515,  3564,  3613,  3663,  3712,  3763,  3813,  3864,  3916,  3968,  4020,  4072,  4125,  4179,  4233,  4287,
     4342,  4397,  4452,  4508,  4564,  4621,  4678,  4735,  4793,  4851,  4910,  4969,  5029,  5089,  5149,  5210,
     5271,  5333,  5395,  5457,  5520,  5583,  5647,  5711,  5776,  5841,  5906,  5972,  6038,  6105,  6172,  6240,
     6308,  6376,  6445,  6514,  6584,  6654,  6724,  6795,  6867,  6939,  7011,  7084,  7157,  7231,  7305,  7379,
     7454,  7529,  7605,  7682,  7758,  7835,  7913,  7991,  8070,  8148,  8228,  8308,  8388,  8469,  8550,  8632,
     8714,  8796,  8879,  8963,  9047,  9131,  9216,  9301,  9387,  9473,  9560,  9647,  9734,  9823,  9911, 10000,
];

/// Computes the WCAG 2.1 relative luminance multiplied by 10,000 for integer arithmetic.
pub const fn relative_luminance_x10000(argb: u32) -> u32 {
    let r_idx = ((argb >> 16) & 0xFF) as usize;
    let g_idx = ((argb >> 8) & 0xFF) as usize;
    let b_idx = (argb & 0xFF) as usize;

    let r_lin = SRGB_TO_LINEAR_X10000[r_idx] as u32;
    let g_lin = SRGB_TO_LINEAR_X10000[g_idx] as u32;
    let b_lin = SRGB_TO_LINEAR_X10000[b_idx] as u32;

    (2126 * r_lin + 7152 * g_lin + 722 * b_lin) / 10000
}

/// Computes the WCAG 2.1 contrast ratio multiplied by 100 (e.g. 450 = 4.5:1).
pub const fn contrast_ratio_x100(color_a: u32, color_b: u32) -> u32 {
    let lum_a = relative_luminance_x10000(color_a);
    let lum_b = relative_luminance_x10000(color_b);

    let (lighter, darker) = if lum_a >= lum_b {
        (lum_a, lum_b)
    } else {
        (lum_b, lum_a)
    };

    // (L1 + 0.05) / (L2 + 0.05) * 100
    // With L in 0..=10000, 0.05 corresponds to 500
    let num = lighter + 500;
    let den = darker + 500;
    (num * 100) / den
}

/// Verifies if two colors satisfy the WCAG 2.1 AA minimum contrast threshold (4.5:1).
#[inline]
pub const fn meets_wcag_aa(fg: u32, bg: u32) -> bool {
    contrast_ratio_x100(fg, bg) >= 450
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::node::{UiBounds, UiPoint, UiRole, UiTree};

    #[test]
    fn test_user_text_scale_levels_and_percentages() {
        assert_eq!(UserTextScale::from_u8(0), UserTextScale::Compact);
        assert_eq!(UserTextScale::from_u8(1), UserTextScale::Normal);
        assert_eq!(UserTextScale::from_u8(2), UserTextScale::Large);
        assert_eq!(UserTextScale::from_u8(3), UserTextScale::ExtraLarge);
        assert_eq!(UserTextScale::from_u8(99), UserTextScale::ExtraLarge); // Clamped

        assert_eq!(UserTextScale::Compact.percentage(), 80);
        assert_eq!(UserTextScale::Normal.percentage(), 100);
        assert_eq!(UserTextScale::Large.percentage(), 125);
        assert_eq!(UserTextScale::ExtraLarge.percentage(), 150);
    }

    /// The stored setting reaches the renderer.
    ///
    /// WHY this is one test and not four: `font::USER_TEXT_SCALE` is one
    /// process-wide cell and the test harness runs test functions on parallel
    /// threads, so a second function that read the published rung would race
    /// with the writes below. Every other check in this module uses the pure
    /// `*_user_scale` arithmetic, which takes its level as an argument and so
    /// cannot be moved by this test.
    ///
    /// WHY the first block is the one that matters: at the default level every
    /// runtime helper has to answer with the ladder's own number, or the
    /// setting would already be resizing screens that never asked for it -- and
    /// every layout constant and contract assertion in the tree is written
    /// against the ladder.
    #[test]
    fn the_published_rung_reaches_the_renderer() {
        // 1. Nothing published yet: the rung is Normal and changes no number.
        assert_eq!(
            UserTextScale::from_u8(crate::font::user_text_scale()),
            UserTextScale::Normal,
            "the published rung must default to the setting that scales nothing"
        );
        for rung in 1..=7usize {
            assert_eq!(font::cap_for_scale_user(rung), font::cap_for_scale(rung));
            assert_eq!(font::em_for_scale_user(rung), font::em_for_scale(rung));
            assert_eq!(font::line_height_user(rung), font::line_height(rung));
            assert_eq!(
                font::ascent_for_scale_user(rung),
                font::ascent_for_scale(rung)
            );
        }
        assert_eq!(font::measure_user(b"Settings", 2), font::measure(b"Settings", 2));
        assert_eq!(
            font::measure_bold_user(b"Settings", 2),
            font::measure_bold(b"Settings", 2)
        );
        assert_eq!(DynamicType::current().scale, UserTextScale::Normal);

        // 2. Publishing a level moves the runtime helpers to the level's own
        // arithmetic, and the row box moves with the cap.
        for (level, dt) in [
            (UserTextScale::Compact, DynamicType::from_level(0)),
            (UserTextScale::Large, DynamicType::from_level(2)),
            (UserTextScale::ExtraLarge, DynamicType::from_level(3)),
            (UserTextScale::Normal, DynamicType::from_level(1)),
        ] {
            assert_eq!(dt.publish(), level, "{level:?} must store as itself");
            for rung in 1..=7usize {
                let want = level.as_u8();
                assert_eq!(
                    font::cap_for_scale_user(rung),
                    font::cap_for_user_scale(rung, want),
                    "{level:?} at rung {rung}: the cap must follow the published rung"
                );
                assert_eq!(
                    font::line_height_user(rung),
                    font::line_height_for_user_scale(rung, want),
                    "{level:?} at rung {rung}: the box must follow the same rung as the ink"
                );
                assert_eq!(
                    font::em_for_scale_user(rung),
                    font::em_for_user_scale(rung, want)
                );
                assert_eq!(
                    font::ascent_for_scale_user(rung),
                    font::ascent_for_user_scale(rung, want)
                );
            }
            assert_eq!(DynamicType::current().scale, level);
        }

        // 3. The largest setting is the ceiling: a level above it is stored as
        // it, so a caller that shows the person what was applied shows the one
        // the platform drew with.
        assert_eq!(
            crate::font::set_user_text_scale(9),
            UserTextScale::ExtraLarge.as_u8()
        );
        assert_eq!(UserTextScale::from_u8(crate::font::user_text_scale()), UserTextScale::ExtraLarge);
        assert_eq!(font::cap_for_scale_user(2), font::cap_for_user_scale(2, 3));

        // 4. Restored, so a later reader of the cell -- a test, or this crate's
        // own host-side checks -- finds the default rather than this test's
        // last write.
        assert_eq!(DynamicType::from_level(1).publish(), UserTextScale::Normal);
        assert_eq!(font::cap_for_scale_user(2), font::cap_for_scale(2));
    }

    /// The store's spelling and the enum's number reach the same rung.
    #[test]
    fn the_settings_value_and_the_level_agree() {
        for (word, want) in [
            (b"compact".as_slice(), UserTextScale::Compact),
            (b"Compact".as_slice(), UserTextScale::Compact),
            (b"COMPACT".as_slice(), UserTextScale::Compact),
            (b"0".as_slice(), UserTextScale::Compact),
            (b"normal".as_slice(), UserTextScale::Normal),
            (b"1".as_slice(), UserTextScale::Normal),
            (b"large".as_slice(), UserTextScale::Large),
            (b"Large".as_slice(), UserTextScale::Large),
            (b"2".as_slice(), UserTextScale::Large),
            (b"extralarge".as_slice(), UserTextScale::ExtraLarge),
            (b"extra_large".as_slice(), UserTextScale::ExtraLarge),
            (b"ExtraLarge".as_slice(), UserTextScale::ExtraLarge),
            (b"3".as_slice(), UserTextScale::ExtraLarge),
        ] {
            assert_eq!(
                UserTextScale::from_settings_value(word),
                Some(want),
                "{:?} must read as {want:?}",
                core::str::from_utf8(word)
            );
        }
        // A value the store does not spell is no level at all: the caller keeps
        // its own default rather than inventing a rung the person never chose.
        for unknown in [b"".as_slice(), b"4".as_slice(), b"huge".as_slice(), b"larg".as_slice()] {
            assert_eq!(UserTextScale::from_settings_value(unknown), None);
        }
    }

    #[test]
    fn test_dynamic_type_scaling_bounds() {
        let dt_compact = DynamicType::from_level(0);
        let dt_normal = DynamicType::from_level(1);
        let dt_large = DynamicType::from_level(2);
        let dt_xl = DynamicType::from_level(3);

        // WHY the rungs are read from the ladder and not written here: this test
        // checks what a person's text-size level does to a rung. A copy of the
        // ladder's own numbers inside it fails every time the ladder is
        // re-measured, which says nothing about the scaling being tested.
        for rung in 1..=6usize {
            let base = font::cap_for_scale(rung);
            assert_eq!(dt_normal.cap_height(rung), base, "normal is the ladder itself");
            assert_eq!(dt_compact.cap_height(rung), (base * 80 + 50) / 100);
            assert_eq!(dt_large.cap_height(rung), (base * 125 + 50) / 100);
            assert_eq!(dt_xl.cap_height(rung), (base * 150 + 50) / 100);

            // Every rung stays legible at the smallest setting, and the levels
            // grow in order.
            assert!(dt_compact.cap_height(rung) >= 10);
            assert!(dt_compact.cap_height(rung) <= dt_normal.cap_height(rung));
            assert!(dt_normal.cap_height(rung) <= dt_large.cap_height(rung));
            assert!(dt_large.cap_height(rung) <= dt_xl.cap_height(rung));
        }

        // The floor is a floor: a rung whose eighty per cent is below ten pixels
        // is drawn at ten rather than at its own percentage.
        assert_eq!(font::scale_cap_height(11, 0), 10);
    }

    #[test]
    fn test_accessibility_node_extraction_and_filtering() {
        // Interactive node with label
        let btn = UiNode::with_label(1, UiRole::Button, UiBounds::new(0, 0, 50, 50), "Save");
        let acc_btn = AccessibilityNode::from_ui_node(&btn).expect("interactive node extracted");
        assert_eq!(acc_btn.label, "Save");
        assert_eq!(acc_btn.role, UiRole::Button);
        assert!(acc_btn.is_focusable());

        // Static container without label: filtered out
        let container = UiNode::new(2, UiRole::Generic, UiBounds::new(0, 0, 100, 100));
        assert!(AccessibilityNode::from_ui_node(&container).is_none());

        // Static text with label: extracted for readout
        let text = UiNode::with_label(3, UiRole::StaticText, UiBounds::new(0, 0, 80, 20), "Status: Online");
        let acc_text = AccessibilityNode::from_ui_node(&text).expect("labeled text extracted");
        assert_eq!(acc_text.label, "Status: Online");
        assert!(!acc_text.is_focusable());
    }

    #[test]
    fn test_accessibility_tree_walker_and_focus() {
        let mut tree = UiTree::new();
        let root = tree.insert(None, UiNode::with_label(0, UiRole::Dialog, UiBounds::new(0, 0, 300, 300), "Settings")).unwrap();

        let wifi = tree.insert(Some(root), UiNode::with_label(0, UiRole::Switch, UiBounds::new(10, 10, 100, 40), "Wi-Fi").with_focused(true)).unwrap();
        let bluetooth = tree.insert(Some(root), UiNode::with_label(0, UiRole::Switch, UiBounds::new(10, 60, 100, 40), "Bluetooth")).unwrap();
        let close = tree.insert(Some(root), UiNode::with_label(0, UiRole::Button, UiBounds::new(10, 120, 80, 40), "Close")).unwrap();

        let walker = AccessibilityTreeWalker::new(&tree);

        // Find focused
        let focused = walker.find_focused().expect("focused node found");
        assert_eq!(focused.id, wifi.0);
        assert_eq!(focused.label, "Wi-Fi");

        // Focus navigation forward
        let next = walker.find_next_focusable(Some(wifi.0)).expect("next focusable found");
        assert_eq!(next.id, bluetooth.0);
        assert_eq!(next.label, "Bluetooth");

        let after_bt = walker.find_next_focusable(Some(bluetooth.0)).expect("next after bt");
        assert_eq!(after_bt.id, close.0);

        // Wrap around
        let wrap = walker.find_next_focusable(Some(close.0)).expect("wrap around to first");
        assert_eq!(wrap.id, wifi.0);

        // Focus navigation backwards
        let prev = walker.find_prev_focusable(Some(close.0)).expect("prev before close");
        assert_eq!(prev.id, bluetooth.0);

        // Point hit testing for screen reader explore by touch
        let hit = walker.node_at_point(UiPoint { x: 15, y: 65 }).expect("hit node found");
        assert_eq!(hit.label, "Bluetooth");

        // Collect nodes array
        let mut nodes = [None; 8];
        let count = walker.collect_nodes(&mut nodes);
        assert_eq!(count, 4); // Settings (Dialog), Wi-Fi, Bluetooth, Close
    }

    #[test]
    fn test_readout_formatter_strings() {
        let mut buf = [0u8; 128];

        // Button
        let btn = UiNode::with_label(1, UiRole::Button, UiBounds::new(0, 0, 40, 40), "Submit");
        let len = ReadoutFormatter::format(&btn, &mut buf);
        assert_eq!(core::str::from_utf8(&buf[..len]).unwrap(), "Submit, button");

        // Focused Checkbox
        let cb = UiNode::with_label(2, UiRole::Checkbox, UiBounds::new(0, 0, 40, 40), "Airplane Mode")
            .with_focused(true);
        let len = ReadoutFormatter::format(&cb, &mut buf);
        assert_eq!(core::str::from_utf8(&buf[..len]).unwrap(), "Airplane Mode, checkbox, focused");

        // Disabled Button
        let dis = UiNode::with_label(3, UiRole::Button, UiBounds::new(0, 0, 40, 40), "Delete")
            .with_enabled(false);
        let len = ReadoutFormatter::format(&dis, &mut buf);
        assert_eq!(core::str::from_utf8(&buf[..len]).unwrap(), "Delete, button, disabled");

        // Spoken announcement struct
        let ann = AccessibilityAnnouncement::from_node(&btn, AnnouncementPriority::Assertive);
        assert_eq!(ann.priority, AnnouncementPriority::Assertive);
        assert_eq!(ann.as_str(), "Submit, button");
    }

    #[test]
    fn test_contrast_ratio_evaluation() {
        // Pure black and white: 21:1 (2100)
        let cr_bw = contrast_ratio_x100(0xFFFFFFFF, 0xFF000000);
        assert_eq!(cr_bw, 2100);
        assert!(meets_wcag_aa(0xFFFFFFFF, 0xFF000000));

        // Light theme primary against white: ~5.16:1 (516)
        let cr_primary = contrast_ratio_x100(0xFF2563EB, 0xFFFFFFFF);
        assert!(cr_primary >= 450, "Primary on white must satisfy WCAG AA (>= 450)");

        // Low contrast test (e.g. gray on slightly different gray)
        let cr_low = contrast_ratio_x100(0xFF888888, 0xFF999999);
        assert!(cr_low < 450);
        assert!(!meets_wcag_aa(0xFF888888, 0xFF999999));
    }
}
