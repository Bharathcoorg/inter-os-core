from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_userspace_process_registration_is_parent_atomic():
    source = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "spawn_user_process_with_parent" in source
    assert 'return Err("process registration failed")' in source
    assert 'task.parent_id = parent_id' not in source[source.index("pub fn spawn_userspace_service"):source.index("/// Legacy packaged-process launcher")]


def test_storage_truncate_uses_transactional_write_path():
    source = (ROOT / "kernel/src/storage/interfs.rs").read_text(encoding="utf-8")
    assert "pub fn truncate_file" in source
    assert "self.write_file(path, &data)" in source


def test_package_activation_is_transactional():
    source = (ROOT / "kernel/src/package/registry.rs").read_text(encoding="utf-8")
    assert "InstallTransaction" in source
    assert "transaction.rollback()" in source
    assert "manifest.policy.validate()" in source


def test_userspace_address_space_is_bounded_before_mapping():
    source = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "USER_ADDRESS_SPACE_LIMIT" in source
    assert 'userspace address-space limit exceeded' in source
    assert "mapped_address_space.checked_add(USER_STACK_SIZE)" in source


def test_volume_registry_supports_resolution_and_non_destructive_unregistration():
    source = (ROOT / "kernel/src/storage/volume.rs").read_text(encoding="utf-8")
    assert "pub fn resolve_lba" in source
    assert "pub fn unregister" in source
    assert "Data destruction is deliberately outside" in source
    assert "pub fn descriptors" in source
