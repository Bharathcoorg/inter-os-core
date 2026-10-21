"""Truth contract for the InterOS terminal application.

A shell that invents a user account, a host name or a privilege level is
describing a machine it has not observed. Every assertion below pins a
specific defect found by reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def terminal_main() -> str:
    return read("apps/terminal/src/main.rs")


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
    """The surface was created, flipped and destroyed through the literal
    slot 6."""
    source = terminal_main()
    assert "surface_create(6," not in source
    assert "surface_flip(6)" not in source
    assert "surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source
