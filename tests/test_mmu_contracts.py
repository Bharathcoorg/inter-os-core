"""Static contracts for the x86_64 userspace MMU/ELF boundary."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MEMORY = (ROOT / "kernel/src/memory.rs").read_text()
USER = (ROOT / "kernel/src/task/user.rs").read_text()


def test_kernel_mappings_only_live_in_higher_half():
    assert "for i in 256..512" in MEMORY
    assert "for i in 1..512" not in MEMORY


def test_user_mapping_enforces_wx():
    assert 'W^X violation: writable user mapping must be non-executable' in MEMORY
    assert "flags.contains(PageTableFlags::WRITABLE)" in MEMORY
    assert "flags.contains(PageTableFlags::NO_EXECUTE)" in MEMORY


def test_elf_loader_rejects_structural_violations():
    assert "e_phentsize != 56" in USER
    assert "ELF entry point is outside the canonical user range" in USER
    assert "ELF segment overlaps the reserved null page" in USER
    assert "ELF segment alignment is not a power of two" in USER
    assert "ELF segment virtual/file alignment mismatch" in USER


def test_elf_loader_rolls_back_leaf_frames_on_mapping_failure():
    assert "map_user_page(page, frame, page_flags)" in USER
    assert "recycle_frame(frame)" in USER


def test_user_stack_is_nx_and_guarded():
    assert "PageTableFlags::NO_EXECUTE" in USER
    assert "Stack guard page" in USER
    assert "USER_STACK_SIZE" in USER


def test_address_space_accounting_is_connected_to_process_limits():
    assert "user_mapped_bytes()" in USER
    assert "set_address_space_usage" in USER
    assert "max_address_space_bytes" in USER
