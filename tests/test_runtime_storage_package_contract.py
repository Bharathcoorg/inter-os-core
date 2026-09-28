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
    assert "segment.flags & 0x13 == 0x12" in source
    assert "segment.payload.is_empty()" in source

def test_tcp_receive_reopens_advertised_window_and_emits_update_ack():
    socket = (ROOT / "kernel/src/net/socket.rs").read_text(encoding="utf-8")
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    assert "conn.rcv_wnd = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len) as u32" in socket
    assert "transmit_tcp_segment_with_window" in syscall
    assert "conn.rcv_wnd.min(u16::MAX as u32) as u16" in syscall

def test_rapp_v3_carries_signed_dependency_metadata_without_overloading_v2():
    source = (ROOT / "kernel/src/package/rapp.rs").read_text(encoding="utf-8")
    assert "RAPP_FORMAT_VERSION_V3" in source
    assert "RAPP_V3_HEADER_SIZE" in source
    assert "pub dependencies:Vec<Dependency>" in source
    assert "VersionRequirement::Exact" in source
    assert "VersionRequirement::Minimum" in source
    assert "VersionRequirement::Major" in source
    assert "policy.dependencies=dependencies.clone()" in source
    assert "build_rapp_v3" in source

def test_tcp_retransmission_uses_bounded_exponential_backoff():
    source = (ROOT / "kernel/src/net/tcp.rs").read_text(encoding="utf-8")
    assert "TCP_RETRANSMIT_MAX_TIMEOUT" in source
    assert "r.rto=(r.rto.saturating_mul(2)).min(TCP_RETRANSMIT_MAX_TIMEOUT)" in source
    assert "now.saturating_sub(r.sent_at)>=r.rto" in source

def test_dhcp_wire_message_parser_validates_cookie_and_core_options():
    source = (ROOT / "kernel/src/net/dhcp.rs").read_text(encoding="utf-8")
    assert "DHCP_COOKIE" in source
    assert "pub struct DhcpMessage" in source
    assert "invalid DHCP magic cookie" in source
    assert "lease_seconds=Some" in source

def test_ipv6_nd_and_router_advertisement_parsers_validate_control_messages():
    source = (ROOT / "kernel/src/net/ipv6.rs").read_text(encoding="utf-8")
    assert "ICMPV6_NEIGHBOR_SOLICITATION" in source
    assert "ICMPV6_NEIGHBOR_ADVERTISEMENT" in source
    assert "Icmpv6Neighbor" in source
    assert "Icmpv6RouterAdvertisement" in source
    assert "invalid ICMPv6 RA option" in source

def test_power_suspend_requires_wake_source_and_has_explicit_resume_state():
    source = (ROOT / "kernel/src/resource/power.rs").read_text(encoding="utf-8")
    assert "enum SuspendState" in source
    assert "enum WakeSource" in source
    assert "if self.wake_mask == 0" in source
    assert "self.state=SuspendState::Suspended" in source
    assert "resume_complete" in source

def test_verified_boot_policy_rejects_rollback_and_gates_update_installation():
    source = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
    assert "struct VerifiedBootPolicy" in source
    assert "rollback_index" in source
    assert "install_with_boot_policy" in source
    assert "policy.accept_version(tx.manifest.version)" in source

def test_graphics_exposes_authoritative_global_buffer_manager():
    source = (ROOT / "kernel/src/gui/mod.rs").read_text(encoding="utf-8")
    assert "pub static BUFFER_MANAGER: Mutex<BufferManager>" in source
    assert "BufferManager::new()" in source

def test_vfs_persists_ownership_and_mode_metadata():
    source = (ROOT / "kernel/src/fs/vfs.rs").read_text(encoding="utf-8")
    assert "pub uid: u32" in source
    assert "pub mode: u16" in source
    assert 'buf[..4].copy_from_slice(b"IVM2")' in source
    assert "pub fn chmod" in source
    assert "pub fn chown" in source

def test_interfs_integrity_checker_validates_bitmap_and_extents():
    source = (ROOT / "kernel/src/storage/interfs.rs").read_text(encoding="utf-8")
    assert "pub fn check_integrity" in source
    assert "marked != referenced[block as usize]" in source

def test_verified_boot_policy_is_monotonic_and_persistent():
    source = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
    assert "pub struct VerifiedBootPolicy" in source
    assert "pub fn accept_version" in source
    assert "pub fn advance" in source
    assert "commit_kind(&version.to_le_bytes(), 9)" in source

def test_tcp_acks_advertise_current_receive_window():
    source = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    assert "to_ack_syn: Option<(u16, u16, u32, u32, u16)>" in source
    assert "c.rcv_wnd.min(u16::MAX as u32) as u16" in source
    assert "transmit_tcp_segment_with_window" in source

def test_shared_process_thread_creation_has_dedicated_guarded_stack():
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "pub fn reserve_thread" in process
    assert "pub fn bind_reserved_thread" in process
    assert "Reservation ownership stays with the caller" in process

def test_reserved_thread_binding_failure_does_not_hide_scheduler_rollback():
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    assert "Reservation ownership stays with the caller" in process
    assert "cancel_thread_reservation(process_id)" in scheduler
    assert "pub fn spawn_user_thread" in scheduler
    assert "map_user_thread_stack" in scheduler
    assert "THREAD_STACK" not in user or "map_user_thread_stack" in user
    assert "guard-page stride" in user

def test_process_leader_reap_preserves_zombie_until_sibling_threads_exit():
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    assert "record.leader_task = TaskId(0)" in process
    assert "record.state = ProcessState::Zombie" in process
    assert "remaining == 0 && record.state == ProcessState::Zombie" in process

def test_tcp_receive_path_uses_strict_sequence_validation():
    tcp = (ROOT / "kernel/src/net/tcp.rs").read_text(encoding="utf-8")
    net = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    assert "pub fn valid_syn_ack" in tcp
    assert "pub fn valid_established_segment" in tcp
    assert "valid_syn_ack(&segment, conn.send_seq.wrapping_add(1))" in net
    assert "valid_established_segment(&segment, conn.recv_seq, conn.snd_una, conn.send_seq)" in net

def test_power_suspend_tracks_domain_ownership():
    source = (ROOT / "kernel/src/resource/power.rs").read_text(encoding="utf-8")
    assert "pub trait PowerDomain" in source
    assert "pub struct PowerDomainMask" in source
    assert "suspended_domains:PowerDomainMask" in source
    assert "PowerDomainMask::all()" in source

def test_shared_process_thread_creation_is_transactional_and_reuses_cr3():
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    assert "pub fn spawn_user_thread" in scheduler
    assert "let shared_space = match self.tasks.iter()" in scheduler
    assert "cancel_thread_reservation(process_id)" in scheduler
    assert "tcb.page_table = Some(shared_space)" in scheduler
    assert "pub fn reserve_thread" in process
    assert "pub fn bind_reserved_thread" in process
    assert "pub fn map_user_thread_stack" in user
    assert "1024 * 1024usize" in user

def test_vfs_hard_links_share_inode_and_persist_aliases():
    source = (ROOT / "kernel/src/fs/vfs.rs").read_text(encoding="utf-8")
    assert "hard_links: BTreeMap<String, InodeId>" in source
    assert "pub fn link(&mut self, old_path: &str, new_path: &str)" in source
    assert "pub fn unlink(&mut self, path: &str)" in source
    assert "buf[pos] = 3" in source
    assert "self.hard_links.insert(path.to_string(), InodeId(id))" in source
    assert "previous_ctime" in source
    assert "self.persist_metadata_snapshot().is_err()" in source
    assert "primary inode record already persists the complete nlink count" in source

def test_vfs_hard_link_restore_does_not_double_count_nlink():
    source = (ROOT / "kernel/src/fs/vfs.rs").read_text(encoding="utf-8")
    restore = source[source.index("fn restore_metadata_snapshot"):]
    assert "kind == 3" in restore
    assert "inode.nlink = inode.nlink.saturating_add(1)" not in restore

def test_userspace_thread_creation_uses_shared_address_space_and_guarded_stack():
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    memory = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
    assert scheduler.count("pub fn spawn_user_thread(") == 1
    assert "shared_space.lock()" in scheduler
    assert "map_user_thread_stack" in scheduler
    assert "reserve_thread(process_id)" in scheduler
    assert "bind_reserved_thread(process_id, id)" in scheduler
    assert "USER_STACK_SIZE + 4096" in user
    assert "pub fn unmap_user_page" in memory
    assert "mapped.drain(..)" in user
    assert "pub fn bind_reserved_thread" in process

def test_process_leader_reap_preserves_zombie_until_sibling_threads_exit():
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    assert "record.leader_task = TaskId(0)" in process
    assert "remaining == 0 && record.state == ProcessState::Zombie" in process
    assert "record.state != ProcessState::Zombie" in process

def test_thread_stack_binding_failure_rolls_back_mapped_stack():
    user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    assert "pub fn unmap_user_thread_stack" in user
    assert "unmap_user_thread_stack(&mut pt, id)" in scheduler
    assert 'return Err("thread stack rollback failed")' in scheduler

def test_page_table_mapping_rollback_unlinks_new_intermediate_tables():
    memory = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
    assert "let mut new_p3 = false" in memory
    assert "let mut new_p2 = false" in memory
    assert "let mut new_p1 = false" in memory
    assert "p2[p2_idx].set_unused()" in memory
    assert "p3[p3_idx].set_unused()" in memory
    assert "pml4[p4_idx].set_unused()" in memory
    assert "caller retains ownership of the leaf frame" in memory

def test_unmap_checks_frame_ownership_before_clearing_the_leaf():
    memory = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
    ownership = memory[memory.index("let frame = p1[p1_idx].frame()"):memory.index("let (active_cr3", memory.index("let frame = p1[p1_idx].frame()"))]
    assert "let Some(pos) = self.allocated_frames.iter().position" in ownership
    assert "p1[p1_idx].set_unused();" in ownership

def test_register_thread_rolls_back_reservation_when_binding_fails():
    process = (ROOT / "kernel/src/task/process.rs").read_text(encoding="utf-8")
    block = process[process.index("pub fn register_thread"):process.index("pub fn thread_started")]
    assert "reserve_thread(process)?" in block
    assert "if let Err(error) = bind_reserved_thread(process, task)" in block
    assert "cancel_thread_reservation(process)" in block
    assert "return Err(error)" in block

def test_ipc_task_cleanup_releases_share_and_shm_ownership():
    share = (ROOT / "kernel/src/ipc/share.rs").read_text(encoding="utf-8")
    shm = (ROOT / "kernel/src/ipc/shm_pipe.rs").read_text(encoding="utf-8")
    ipc = (ROOT / "kernel/src/ipc/mod.rs").read_text(encoding="utf-8")
    assert "pub fn cleanup_task(&mut self, task_id: TaskId) -> usize" in share
    assert "target.target_task != task_id" in share
    assert "pub fn cleanup_task(&mut self, task_id: TaskId) -> usize" in shm
    assert "p.producer == task_id || p.consumer == task_id" in shm
    assert "SHARE_SHEET_MANAGER.lock().cleanup_task(task_id)" in ipc
    assert "SHM_PIPE_MANAGER.lock().cleanup_task(task_id)" in ipc

def test_task_teardown_releases_network_ownership():
    net = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
    assert "pub fn cleanup_task(task_id: TaskId)" in net
    assert "SOCKET_TABLE.lock().release_owner(task_id.0)" in net
    assert "TCP_TABLE.lock().release_owner(task_id.0)" in net
    assert "crate::net::cleanup_task(task_id)" in scheduler

def test_verified_service_launch_reuses_one_generation_transition():
    services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
    launch = services[services.index("pub fn launch_verified_package"):services.index("/// Stops a service task", services.index("pub fn launch_verified_package"))]
    assert "let generation = existing.map(|r| r.generation.saturating_add(1)).unwrap_or(1);" in launch
    assert "a successful" in launch
    assert "ServiceRecord { id, task, state: ServiceState::Running, generation }" in launch
