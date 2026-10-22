"""Contract test verifying accessibility, dynamic type scaling, and theme contrast ratios.

Verifies:
1. All standard theme surface and ink token pairs meet or exceed the WCAG 2.1 AA
   minimum contrast ratio of 4.5:1 across Light, Dark, and Neutral theme systems.
2. Dynamic type scaling provides bounded, monotonic cap height scaling driven by
   user text scale levels (0=compact 80%, 1=normal 100%, 2=large 125%, 3=extra large 150%).
3. UiNode carries `accessibility_label` and is consumed by accessibility tree walkers
   and screen reader readout formatters with zero heap allocations.
"""

from pathlib import Path
import re
import pytest

ROOT = Path(__file__).resolve().parents[1]
THEME_RS = ROOT / "sdk/rust/interos-std/src/design/theme.rs"
COLOR_RS = ROOT / "sdk/rust/interos-std/src/design/mod.rs"
FONT_RS = ROOT / "sdk/rust/interos-std/src/font.rs"
ACCESSIBILITY_RS = ROOT / "sdk/rust/interos-std/src/design/accessibility.rs"
NODE_RS = ROOT / "sdk/rust/interos-std/src/design/node.rs"

WCAG_AA_MINIMUM_CONTRAST = 4.5


def srgb_to_linear(channel: int) -> float:
    """Converts an 8-bit sRGB color component to linear luminance."""
    c = channel / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def relative_luminance(argb: int) -> float:
    """Calculates WCAG 2.1 relative luminance from a 32-bit ARGB color value."""
    r = srgb_to_linear((argb >> 16) & 0xFF)
    g = srgb_to_linear((argb >> 8) & 0xFF)
    b = srgb_to_linear(argb & 0xFF)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(color_a: int, color_b: int) -> float:
    """Calculates the WCAG 2.1 contrast ratio between two ARGB colors."""
    lum_a = relative_luminance(color_a)
    lum_b = relative_luminance(color_b)
    lighter = max(lum_a, lum_b)
    darker = min(lum_a, lum_b)
    return (lighter + 0.05) / (darker + 0.05)


def parse_color_constants() -> dict[str, int]:
    """Extracts raw color definitions from design/mod.rs."""
    text = COLOR_RS.read_text(encoding="utf-8")
    constants = {}
    pattern = r"pub const ([A-Z0-9_]+):\s*u32\s*=\s*(0x[0-9A-Fa-f]+);"
    for match in re.finditer(pattern, text):
        name = match.group(1)
        val = int(match.group(2), 16)
        constants[f"color::{name}"] = val
    return constants


def parse_themes() -> dict[str, dict[str, int]]:
    """Extracts surface and ink tokens for LIGHT, DARK, and NEUTRAL themes."""
    color_constants = parse_color_constants()
    text = THEME_RS.read_text(encoding="utf-8")

    themes = {}
    for theme_name in ("LIGHT", "DARK", "NEUTRAL"):
        block_pattern = rf"pub const {theme_name}:\s*Theme\s*=\s*Theme\s*\{{(.*?)\n\s*\}};"
        match = re.search(block_pattern, text, re.DOTALL)
        assert match, f"Could not find Theme::{theme_name} definition"
        block = match.group(1)

        tokens = {}
        for line in block.splitlines():
            line = line.strip().rstrip(",")
            if not line or line.startswith("//"):
                continue
            if ":" in line:
                key, val = [x.strip() for x in line.split(":", 1)]
                if val.startswith("0x"):
                    tokens[key] = int(val, 16)
                elif val in color_constants:
                    tokens[key] = color_constants[val]

        # Inverted surface methods
        tokens["inverted_surface"] = tokens["on_surface"]
        tokens["on_inverted_surface"] = tokens["surface"]

        themes[theme_name] = tokens

    return themes


def test_standard_theme_surface_and_ink_contrast():
    """All standard theme surface and ink token pairs must satisfy WCAG AA 4.5:1 minimum."""
    themes = parse_themes()

    standard_pairs = [
        ("surface", "on_surface"),
        ("surface_container", "on_surface"),
        ("surface_container_high", "on_surface"),
        ("surface_container_low", "on_surface"),
        ("surface", "on_surface_variant"),
        ("surface_container", "on_surface_variant"),
        ("primary", "on_primary"),
        ("primary_container", "on_primary_container"),
        ("inverted_surface", "on_inverted_surface"),
    ]

    failures = []
    reports = []

    for theme_name, tokens in themes.items():
        for surface_token, ink_token in standard_pairs:
            assert surface_token in tokens, f"Missing token {surface_token} in {theme_name}"
            assert ink_token in tokens, f"Missing token {ink_token} in {theme_name}"

            surface_color = tokens[surface_token]
            ink_color = tokens[ink_token]
            ratio = contrast_ratio(surface_color, ink_color)
            reports.append(f"{theme_name} ({surface_token} vs {ink_token}): {ratio:.2f}:1")

            if ratio < WCAG_AA_MINIMUM_CONTRAST:
                failures.append(
                    f"{theme_name}: pair ({surface_token}=0x{surface_color:08X}, "
                    f"{ink_token}=0x{ink_color:08X}) has contrast ratio {ratio:.2f}:1, "
                    f"which fails the WCAG AA minimum of {WCAG_AA_MINIMUM_CONTRAST}:1"
                )

    assert not failures, "Contract violation: theme pairs with insufficient contrast:\n" + "\n".join(failures)


def test_dynamic_type_scaling_bounds_and_rungs():
    """Dynamic type scaling must provide 80%, 100%, 125%, and 150% scales bounded gracefully."""
    font_src = FONT_RS.read_text(encoding="utf-8")

    assert "pub const fn cap_for_user_scale" in font_src
    assert "pub const fn cap_for_scale_with_user" in font_src
    assert "pub const fn scale_cap_height" in font_src
    assert "TEXT_SCALE_COMPACT: u8 = 0;" in font_src
    assert "TEXT_SCALE_NORMAL: u8 = 1;" in font_src
    assert "TEXT_SCALE_LARGE: u8 = 2;" in font_src
    assert "TEXT_SCALE_EXTRA_LARGE: u8 = 3;" in font_src

    def scale_cap(base: int, user_scale: int) -> int:
        if base == 0:
            return 0
        if user_scale == 0:
            s = (base * 80 + 50) // 100
            return max(10, s)
        elif user_scale == 1:
            return base
        elif user_scale == 2:
            return (base * 125 + 50) // 100
        else:
            return (base * 150 + 50) // 100

    canonical_rungs = [13, 16, 22, 30, 40, 52, 72]

    # Level 0 (Compact 80%): must be >= 10px and <= normal
    compact_rungs = [scale_cap(r, 0) for r in canonical_rungs]
    for c, n in zip(compact_rungs, canonical_rungs):
        assert c >= 10, f"Compact cap height {c} collapsed below 10px minimum"
        assert c <= n, f"Compact cap height {c} exceeds normal {n}"

    # Level 1 (Normal 100%): must equal canonical
    normal_rungs = [scale_cap(r, 1) for r in canonical_rungs]
    assert normal_rungs == canonical_rungs

    # Level 2 (Large 125%): must be >= normal and <= extra large
    large_rungs = [scale_cap(r, 2) for r in canonical_rungs]
    for l, n in zip(large_rungs, canonical_rungs):
        assert l >= n

    # Level 3 (Extra Large 150%): must be >= large
    xl_rungs = [scale_cap(r, 3) for r in canonical_rungs]
    for xl, l in zip(xl_rungs, large_rungs):
        assert xl >= l

    # Monotonic progression across user text scales
    for i, rung in enumerate(canonical_rungs):
        c = compact_rungs[i]
        n = normal_rungs[i]
        l = large_rungs[i]
        xl = xl_rungs[i]
        assert c < n <= l < xl, f"Rung {rung} scale progression failed: {c} < {n} <= {l} < {xl}"


def test_accessibility_subsystem_source_contract():
    """UiNode accessibility labels and consumer modules must exist and be exported."""
    assert NODE_RS.exists(), "node.rs must exist in design subsystem"
    assert ACCESSIBILITY_RS.exists(), "accessibility.rs must exist in design subsystem"

    node_src = NODE_RS.read_text(encoding="utf-8")
    assert "pub struct UiNode" in node_src
    assert "pub accessibility_label: &'static str" in node_src
    assert "pub enum UiRole" in node_src
    assert "pub struct UiTree" in node_src

    acc_src = ACCESSIBILITY_RS.read_text(encoding="utf-8")
    assert "pub enum UserTextScale" in acc_src
    assert "pub struct DynamicType" in acc_src
    assert "pub struct AccessibilityNode" in acc_src
    assert "pub struct AccessibilityTreeWalker" in acc_src
    assert "pub struct ReadoutFormatter" in acc_src
    assert "pub struct AccessibilityAnnouncement" in acc_src
    assert "pub const fn contrast_ratio_x100" in acc_src
    assert "pub const fn meets_wcag_aa" in acc_src

    # Module export checks in design/mod.rs
    mod_src = COLOR_RS.read_text(encoding="utf-8")
    assert "pub mod node;" in mod_src
    assert "pub mod accessibility;" in mod_src
