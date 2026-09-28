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


def test_graphics_frame_fence_requires_explicit_signal_before_scanout():
    source = (ROOT / "kernel/src/gui/buffer.rs").read_text(encoding="utf-8")
    assert "pub fn signal" in source
    assert 'frame fence not signaled' in source
    assert "b.fence.signaled=false" in source


def test_ipv6_autoconfiguration_rejects_non_64bit_interface_prefixes():
    source = (ROOT / "kernel/src/net/ipv6.rs").read_text(encoding="utf-8")
    assert "pub fn valid(&self)->bool" in source
    assert "self.prefix.prefix_len == 64" in source
    assert "pub fn address(&self)->Option<Ipv6Address>" in source


def test_security_registries_fail_closed_at_persistence_capacity():
    source = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    assert 'publisher trust store is full' in source
    assert 'package revocation registry is full' in source
    assert 'key revocation registry is full' in source
    assert 'failed to persist key rotation' in source


def test_power_policy_does_not_publish_refresh_rate_before_telemetry():
    source = (ROOT / "kernel/src/resource/power.rs").read_text(encoding="utf-8")
    assert "thermal_valid: bool" in source
    assert "battery_valid: bool" in source
    assert 'if !self.telemetry_valid' in source
    assert "self.display_refresh_hz = 0" in source


def test_update_activation_metadata_is_persisted_before_backend_switch():
    source = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
    assert "PersistenceFailed" in source
    assert "if activation.persist().is_err()" in source
    assert "backend.activate_slot" in source


def test_animation_deadline_is_overflow_safe():
    source = (ROOT / "kernel/src/gui/ui.rs").read_text(encoding="utf-8")
    assert "saturating_add(self.duration_ms as u64)" in source


def test_userspace_loader_does_not_embed_first_party_images_directly():
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    images = (ROOT / "kernel/src/task/images.rs").read_text(encoding="utf-8")
    task_mod = (ROOT / "kernel/src/task/mod.rs").read_text(encoding="utf-8")
    assert "include_bytes!" not in user
    assert "pub mod images;" in task_mod
    assert "pub fn service_image" in images
    assert "spawn_user_process_with_parent(image.name, image.elf" in user


def test_package_launch_requires_exact_active_version_and_active_dependencies():
    source = (ROOT / "kernel/src/package/registry.rs").read_text(encoding="utf-8")
    assert "matches!(entry.state, PackageState::Active)" in source
    assert "version == e.version" in source


def test_repository_does_not_track_generated_application_elfs():
    gitignore = (ROOT / ".gitignore").read_text(encoding="utf-8")
    assert "apps/*/bin/" in gitignore
    assert "!apps/test_app/bin/test_app.elf" not in gitignore
    assert not (ROOT / "apps/test_app/bin/test_app.elf").exists()


def test_service_capability_policy_is_authoritative_for_userspace_launch():
    services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "pub struct ServiceCapabilitySpec" in services
    assert "pub fn capability_policy" in services
    assert "crate::services::capability_policy(service, parent_id)" in user
    assert "CapabilityType::ProcessKill" not in user[user.index("pub fn spawn_userspace_service"):user.index("/// Legacy packaged-process launcher")]



def test_service_discovery_exposes_stable_metadata_without_task_identity():
    services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    assert "pub struct ServiceInfo" in services
    assert "pub fn lookup(service_id: u8)" in services
    assert "generation" in services
    assert "ServiceQuery = 102" in syscall
    assert "pub fn sys_service_query" in syscall
    assert "pub fn service_query" in sdk
    assert "task identity" in syscall



def test_platform_documentation_does_not_claim_unverified_qemu_or_hardware_validation():
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    assert "Runtime execution is actively verified across three primary emulation targets" not in readme
    assert "| Primary emulation contract; runtime validation pending |" in readme
    assert "| Source-level architecture target; runtime validation pending |" in readme



def test_process_thread_registry_binds_task_identity_to_process():
    source = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    assert "pub const MAX_THREADS: usize = 1024;" in source
    assert "pub struct ThreadRecord" in source
    assert "pub static THREAD_TABLE" in source
    assert "THREAD_TABLE.lock().iter().flatten().find(|r| r.task == task)" in source
    assert "pub fn register_thread(process: ProcessId, task: TaskId)" in source
    assert "leader_binding" in source


def test_native_apps_use_atomic_parent_assignment_at_spawn():
    source = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "task.parent_id = id_security" not in source
    assert source.count("spawn_user_process_with_parent(") >= 13


def test_tcp_transport_tracks_peer_window_and_congestion_state():
    source = (ROOT / "kernel/src/net/socket.rs").read_text(encoding="utf-8")
    assert "snd_una" in source
    assert "snd_wnd" in source
    assert "cwnd" in source
    assert "ssthresh" in source
    assert "dup_acks" in source
    assert "pub fn update_peer_window" in source
    assert "pub fn send_window" in source
    source2 = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    assert "send_window(connection_id)" in source2


def test_tcp_partial_acknowledgements_trim_retransmission_buffer():
    source = (ROOT / "kernel/src/net/socket.rs").read_text(encoding="utf-8")
    assert "conn.tx.copy_within(acknowledged..conn.tx_len, 0)" in source
    assert "conn.tx_seq = ack" in source
    assert "conn.tx_len -= acknowledged" in source


def test_service_public_snapshot_does_not_expose_scheduler_task_ids():
    source = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    snapshot = source[source.index("pub fn snapshot"):source.index("/// Returns the most recent heartbeat")]
    assert "record.task.0" not in snapshot
    assert '"{}|{}|{}|{}|{}|{}"' in snapshot


def test_tcp_transport_advertises_actual_receive_window_on_handshake_data_and_retransmit():
    net = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    assert "transmit_tcp_segment_with_window" in net
    assert "transmit_tcp_syn_with_window" in net
    assert "send_tcp_payload_with_window" in net
    assert "rcv_wnd.min(u16::MAX as u32) as u16" in net
    assert "send_tcp_syn_with_window" in syscall
    assert "send_tcp_payload_with_window" in syscall
    assert "transmit_tcp_segment_with_window" in syscall


def test_tcp_syn_ack_requires_exact_ack_and_nonempty_sequence_space():
    source = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    assert "let expected_ack = conn.send_seq.wrapping_add(1);" in source
    assert "segment.ack == expected_ack" in source
    assert "segment.payload.is_empty()" in source
    assert "segment.seq != 0" in source
