"""Contract tests for the InterOS design system.

WHY this exists: the SDK carried design tokens and no app used them. Every app
wrote its palette out in hex literals of its own, and the launcher kept a
*private duplicate* of the token set in `apps/launcher/src/theme.rs` with
values that contradicted the SDK's -- the SDK's radii were 8/12/18/24/32/999 and
the launcher's were 10/18/24/32/44, the SDK's minimum touch target was 44 and
the launcher's was 48, and the SDK had a type scale in `typography` while the
launcher had a `type_scale` of its own. The UI goals were therefore only half
enforced, and a third-party app had nothing to inherit.

The SDK now carries a theme -- roles rather than colours -- and the launcher
resolves its colours, radii, spacing, type scale and touch target through it.
These tests pin that arrangement, so the duplicate cannot come back.

Each test asserts an *absence* as well as a presence, because a test that only
checks that the new thing exists passes just as happily when the old thing
stays.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT / "sdk" / "rust" / "interos-std" / "src" / "design"
SDK_THEME = SDK / "theme.rs"
SDK_COMPONENTS = SDK / "components.rs"
SDK_MOD = SDK / "mod.rs"
LAUNCHER = ROOT / "apps" / "launcher" / "src"

# The roles an interface asks for by name.
#
# WHY this list and not "whatever the theme happens to have": the list is the
# contract a third-party app builds against. A role that is not named here is
# not one an app may rely on, and a role that is named here and then removed
# breaks every app that used it.
REQUIRED_ROLES = (
    "surface",
    "surface_container",
    "surface_container_high",
    "on_surface",
    "on_surface_variant",
    "primary",
    "on_primary",
    "primary_container",
    "on_primary_container",
    "secondary",
    "outline",
    "outline_variant",
    "error",
    "success",
    "warning",
    "muted",
    "accent",
    "border",
)

# The roles that are the same in both variants, and why.
#
# WHY the two polarity helpers: they describe the surface a pixel lands on
# rather than the theme it came from, so they cannot differ by theme.
# WHY the emergency red: an alarm that changes hue with the theme is an alarm
# that can be mistaken for an ordinary error.
SAME_IN_BOTH_VARIANTS = ("ink_on_light", "ink_on_dark", "emergency")

# The launcher's historical palette, from the theme module it used to carry.
#
# WHY these names are listed rather than a general "no constants" rule: these
# are the names the audit found, and a name that reappears is the old palette
# coming back. The rule is about the *names* the launcher's own palette used.
LEGACY_PALETTE_NAMES = (
    "BG_DARK",
    "SURFACE_DARK",
    "SURFACE_RAISED",
    "SURFACE_LIGHT",
    "BORDER_SUBTLE",
    "TEXT_PRIMARY",
    "TEXT_MUTED",
    "TEXT_DARK",
    "ACCENT_SKY",
    "ACCENT_AMBER",
    "ACCENT_CYAN",
    "ACCENT_VIOLET",
    "SUCCESS_EMERALD",
    "DANGER_CRIMSON",
    "LOCK_BACKDROP_TOP",
    "LOCK_BACKDROP_BOTTOM",
    "TEXT_CHARCOAL",
    "TEXT_SUBTITLE_GRAY",
    "CARD_FROSTED_CREAM",
    "CARD_WARM_OFFWHITE",
    "WEATHER_SUN_AMBER",
    "DIVIDER_HAIRLINE",
)

# A colour constant: a named value written as a literal.
COLOUR_CONSTANT = re.compile(
    r"(?:pub\s+)?const\s+([A-Z_0-9]+)\s*:\s*u32\s*=\s*(0x[0-9A-Fa-f]{8})"
)

# A colour literal anywhere in a source file.
COLOUR_LITERAL = re.compile(r"0x[0-9A-Fa-f]{8}")

# A role assignment inside a theme variant: `role: expression,`.
ROLE_ASSIGNMENT = re.compile(r"(\w+):\s*([^,\n]+),")

# A `u32` constant in the SDK's design module.
SDK_U32_CONSTANT = re.compile(r"pub const (\w+): u32 = (0x[0-9A-Fa-f]{8});")


def _design_module() -> str:
    return SDK_MOD.read_text(encoding="utf-8")


def _theme_source() -> str:
    return SDK_THEME.read_text(encoding="utf-8")


def _variant_block(source: str, name: str) -> str:
    """The body of `Theme::LIGHT` or `Theme::DARK`."""
    marker = f"pub const {name}: Theme = Theme {{"
    assert marker in source, f"the theme has no {name} variant"
    return source.split(marker, 1)[1].split("};", 1)[0]


def _palette() -> dict[str, int]:
    """The SDK's palette, by name.

    WHY the first occurrence wins: the wide-gamut `p3` module repeats several
    names at different values, and the theme refers to the standard-gamut ones.
    """
    values: dict[str, int] = {}
    for name, literal in SDK_U32_CONSTANT.findall(_design_module()):
        values.setdefault(name, int(literal, 16))
    return values


def _role_value(expression: str, palette: dict[str, int]) -> int | None:
    """The value a role assignment resolves to, or None if it is not a colour."""
    expression = expression.strip()
    if re.fullmatch(r"0x[0-9A-Fa-f]{8}", expression):
        return int(expression, 16)
    match = re.fullmatch(r"color::(\w+)", expression)
    if match:
        return palette.get(match.group(1))
    return None


def _role_values(name: str, palette: dict[str, int]) -> dict[str, int]:
    """Every colour role of one theme variant, by role name."""
    values: dict[str, int] = {}
    for role, expression in ROLE_ASSIGNMENT.findall(_variant_block(_theme_source(), name)):
        value = _role_value(expression, palette)
        if value is not None:
            values[role] = value
    return values


def _launcher_sources() -> list[Path]:
    return [source for source in sorted(LAUNCHER.rglob("*.rs")) if "target" not in source.parts]


def _launcher_text() -> str:
    return "\n".join(source.read_text(encoding="utf-8") for source in _launcher_sources())


def test_the_sdk_exposes_a_theme_with_the_platform_roles_in_both_variants():
    """The theme carries every role, in a light and a dark variant.

    WHY this is a source check and not a Rust test: a third-party app reads
    the SDK's source to find out what it may ask for, so the roles have to be
    *declared* roles rather than an accident of one implementation.
    """
    design = _design_module()
    assert "pub mod theme;" in design, "the SDK has no theme module"

    source = _theme_source()
    declared = set(re.findall(r"pub (\w+): u32,", source))
    for role in REQUIRED_ROLES:
        assert role in declared, f"the theme does not declare the role {role}"

    palette = _palette()
    light = _role_values("LIGHT", palette)
    dark = _role_values("DARK", palette)
    for role in REQUIRED_ROLES:
        assert role in light, f"the light theme does not assign {role}"
        assert role in dark, f"the dark theme does not assign {role}"
        assert light[role] != 0, f"{role} is unset in the light theme"
        assert dark[role] != 0, f"{role} is unset in the dark theme"

    for role in set(light) | set(dark):
        if role in SAME_IN_BOTH_VARIANTS:
            continue
        assert light.get(role) != dark.get(role), (
            f"{role} resolves to the same colour in both variants, so the dark "
            "theme is a rename of the light one"
        )


def test_the_sdk_theme_carries_the_scale_the_spacing_the_radii_and_the_touch_minimum():
    """A theme is the whole visual language, not only its colours.

    WHY: an app that asks the theme for a surface and then reaches for a
    spacing step from somewhere else is still assembling itself out of two
    systems, and the two systems drift.
    """
    source = _theme_source()
    for field in ("type_scale", "spacing", "radii", "touch"):
        assert f"pub {field}:" in source, f"the theme does not carry {field}"

    # The metrics are views of the platform's scales, not copies of them.
    assert "radius::CONTROL" in source, "the theme's radii are not the platform's radii"
    assert "spacing::MD" in source, "the theme's spacing is not the platform's spacing"
    assert "touch::MIN_TARGET" in source, "the theme's touch minimum is not the platform's"
    assert "typography::BODY" in source, "the theme's type scale is not the platform's"


def test_the_launcher_does_not_carry_a_palette_of_its_own():
    """The launcher's theme module is an adapter, not a second design system.

    WHY this asserts absences: the audit found the launcher holding its own
    copy of the token set. A test that only checked that the SDK gained a
    theme would have passed while that copy was still there.
    """
    theme_module = (LAUNCHER / "theme.rs").read_text(encoding="utf-8")

    assert not COLOUR_LITERAL.search(theme_module), (
        "the launcher's theme module still carries colour literals: it is a "
        "palette, not an adapter"
    )
    assert "struct Theme" not in theme_module, (
        "the launcher still declares its own Theme: there are two themes"
    )
    assert "impl Theme" not in theme_module, (
        "the launcher still implements its own Theme: there are two themes"
    )
    assert "interos_std::design::theme" in theme_module, (
        "the launcher's theme module does not resolve through the SDK's theme"
    )

    launcher = _launcher_text()
    for name in LEGACY_PALETTE_NAMES:
        assert not re.search(rf"\bconst\s+{name}\b", launcher), (
            f"the launcher still defines the legacy palette constant {name}"
        )


def test_the_launcher_does_not_re_declare_a_colour_the_platform_names():
    """No launcher constant restates a colour the platform already states.

    WHY this is a value check and not a name check: a name check cannot tell
    a screen's own art direction from a re-declaration of the platform's, and
    a screen that paints its own splash colours is not breaking the design
    system. A value check says exactly what it means -- the launcher does not
    state a colour the platform states.

    WHY this can still fail: a colour the platform adds later that happens to
    be one of the launcher's own values is a collision the launcher has to
    answer, and the answer is to ask the platform for it.
    """
    palette = _palette()
    platform = set(palette.values())
    platform |= set(_role_values("LIGHT", palette).values())
    platform |= set(_role_values("DARK", palette).values())

    for source in _launcher_sources():
        text = source.read_text(encoding="utf-8")
        for name, literal in COLOUR_CONSTANT.findall(text):
            value = int(literal, 16)
            assert value not in platform, (
                f"{source.name} declares {name} = {literal}, which is a colour "
                "the platform already names: the launcher should ask the "
                "platform for it"
            )


def test_the_sdk_and_the_launcher_agree_on_the_minimum_touch_target():
    """The minimum touch target is the platform's, and the shell's is the same.

    WHY: the launcher's minimum was 48 and the platform's was 44, so a
    third-party app that drew a 44px target was off-spec against the shell
    that shipped beside it.
    """
    design = _design_module()
    assert "pub const MIN_TARGET: usize = 44;" in design, (
        "the platform's minimum touch target is not 44"
    )

    theme_module = (LAUNCHER / "theme.rs").read_text(encoding="utf-8")
    assert "design::touch::MIN_TARGET" in theme_module, (
        "the launcher does not take its minimum touch target from the platform"
    )
    assert not re.search(r"MIN_TOUCH_TARGET\s*:\s*usize\s*=\s*\d", theme_module), (
        "the launcher still declares its own minimum touch target"
    )


def test_the_sdk_and_the_launcher_agree_on_the_corner_radii():
    """One radius scale, in the SDK, used by the launcher.

    WHY: the two scales carried the same names at different values -- the
    SDK's `CONTROL` was 12 and the launcher's was 18 -- so a control drawn
    against the SDK's scale and a control drawn against the launcher's sat
    beside each other with visibly different corners.
    """
    design = _design_module()
    for declaration in (
        "pub const SMALL: usize = 8;",
        "pub const CHIP: usize = 10;",
        "pub const CONTROL: usize = 12;",
        "pub const CARD: usize = 18;",
        "pub const SHEET: usize = 24;",
        "pub const MAJOR: usize = 32;",
        "pub const SCREEN: usize = 44;",
        "pub const PILL: usize = 999;",
    ):
        assert declaration in design, f"the platform's radius scale changed: {declaration}"

    theme_module = (LAUNCHER / "theme.rs").read_text(encoding="utf-8")
    assert "pub use interos_std::design::radius;" in theme_module, (
        "the launcher does not resolve its radii through the platform's scale"
    )
    assert not re.search(r"pub const \w+\s*:\s*usize\s*=\s*\d+;", theme_module), (
        "the launcher still declares its own radius scale"
    )


def test_the_sdk_gives_a_third_party_app_the_widgets_to_look_native():
    """Tokens alone are not an interface: the SDK ships the widgets too.

    WHY: a third-party app that has to know the platform's corner radius, its
    hairline colour, its elevation steps and its type scale all at once is an
    app that has to read another app's source to look native.
    """
    assert SDK_COMPONENTS.exists(), "the SDK ships no components module"
    components = SDK_COMPONENTS.read_text(encoding="utf-8")
    for widget in (
        "pub fn fill_rounded_rect",
        "pub fn card",
        "pub fn pill",
        "pub fn segmented_control",
        "pub fn app_icon_tile",
        "pub fn list_row",
    ):
        assert widget in components, f"the SDK does not ship {widget}"

    design = _design_module()
    assert "pub mod components;" in design, "the components module is not exported"
