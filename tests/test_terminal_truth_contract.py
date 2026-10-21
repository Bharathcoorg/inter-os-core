"""Truth contract for the InterOS terminal application.

A shell that invents a user account, a host name or a privilege level is
describing a machine it has not observed. Every assertion below pins a
specific defect found by reading the app.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The SDK functions that used to take a capability slot as their first
# argument. The kernel now authorises every one of them by the
# capability's *type*, so the app names the authority it wants and the
# kernel finds it.
SLOT_ARG_SDK_CALLS = (
    "app_storage_open",
    "display_set_mode",
    "display_set_orientation",
    "fs_list",
    "fs_open",
    "fs_read",
    "fs_stat",
    "fs_unlink",
    "fs_write",
    "get_ticks",
    "ipc_recv",
    "ipc_send",
    "log",
    "package_install",
    "package_list",
    "package_registry",
    "package_rollback",
    "package_uninstall",
    "service_package_launch",
    "service_package_verify",
    "settings_get",
    "settings_set",
    "surface_create",
    "surface_destroy",
    "surface_flip",
    "surface_write",
    "verify_package",
    "window_focus",
    "window_set_geometry",
    "window_set_input",
)

# A capability slot named in a comment or in a user-visible label is not
# a call, so the call patterns are matched against the code with both
# removed.
_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)


def code_only(source: str) -> str:
    """Return `source` with comments and string literals removed."""
    return _COMMENT_OR_LITERAL.sub(" ", source)


def slot_arguments(source: str) -> list[str]:
    """Return the SDK calls in `source` that still take a capability slot.

    A slot argument is either a bare number or the name of a resolved
    slot, and a call that names one is a call that pinned a position in
    the task's capability table.
    """
    code = code_only(source)
    return [
        name
        for name in SLOT_ARG_SDK_CALLS
        if re.search(
            r"\b" + name + r"\s*\(\s*(?:\d|[A-Za-z_][A-Za-z0-9_]*_slot\b|\bslot\b)",
            code,
        )
    ]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def terminal_main() -> str:
    return read("apps/terminal/src/main.rs")


def terminal_source() -> str:
    """Everything the terminal app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/terminal/src").rglob("*.rs"))
    )


def test_the_prompt_names_nothing_the_shell_has_not_observed():
    """The prompt was "interos@sovereign:~$".

    That asserted a user account and a host name the shell had never read.
    """
    source = terminal_main()
    assert "interos@sovereign" not in source
    assert 'pub const PROMPT: &[u8] = b"interos:$ ";' in source


def test_the_capability_command_is_an_inspection():
    """It said "statement only; no inspection API".

    The platform resolves a capability slot by its type, so the shell can
    ask which of the capabilities it knows about it actually holds. There
    is no way to enumerate a whole table, and the command says so.
    """
    source = terminal_main()
    assert "no inspection API" not in source
    assert "std::cap_lookup(*cap_type, 0).is_ok()" in source
    assert "pub const KNOWN_CAPABILITIES" in source


def test_the_line_wrapper_does_not_repeat_a_character():
    """`wrap_text` skips the leading spaces of the text it is given.

    The wrapper advanced the remainder by the piece's length, which
    restarted it one character too early, so every wrapped line repeated
    the first character of the next one.
    """
    source = terminal_main()
    assert "rest = &rest[piece[0].len()..];" not in source
    assert "let consumed = skip + piece[0].len();" in source
    assert "rest = &rest[consumed..];" in source


def test_the_filesystem_commands_target_a_path_the_app_may_read():
    """The kernel's app path policy refuses `/data` before the path is
    examined.

    Listing and reading it could only ever produce a refusal, and the
    refusal was reported as an empty directory.
    """
    source = terminal_main()
    assert 'const LS_PATH: &str = "/storage";' in source
    assert "std::fs_list(3," not in source
    assert "std::fs_open(3," not in source
    assert "std::fs_read(3," not in source
    # The path is the user's to give now.
    assert "fn list_path(&mut self, path: &str)" in source
    assert "fn cat_path(&mut self, path: &str)" in source


def test_the_help_lists_every_command_the_dispatcher_answers():
    """The dispatcher also answers `vfs` and `clear`.

    The old list left them out, so the help was a worse description of the
    shell than the shell itself.
    """
    source = terminal_main()
    assert "ls, vfs, services, cat, clear" in source


def test_a_capability_slot_is_resolved_by_its_type():
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises every platform call by the capability's *type* and the
    SDK's functions no longer take a slot argument, so there is nothing
    left for the app to resolve. Pinning a slot number pinned an
    accident: the launcher proved it by passing slot 0 while its
    `FileSystemRead` capability sat at slot 3, so the developer-terminal
    flag it wanted was never read and the refusal reached the user with
    nothing on screen to say why.
    """
    source = terminal_source()
    code = code_only(source)
    # No platform call takes a capability slot any more.
    assert slot_arguments(source) == []
    # The app does not ask the SDK for a slot.
    assert "std::cap::" not in source
    # The one capability question the shell is left with is a yes/no one --
    # "do I hold this capability type?" -- and its answer is never bound to
    # a name and handed to a platform call.
    assert re.search(r"=\s*std::cap_lookup\(", code) is None
    for match in re.finditer(r"std::cap_lookup\(", code):
        assert re.match(r"[^\n;]*\.is_ok\(\)", code[match.end():])
    # The console still opens a surface, presents it and tears it down.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source
