from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_canonical_qemu_exposes_native_portrait_display_and_absolute_pointer():
    launch = read("qemu/launch_studio.ps1")
    server = read("emulator/server.py")
    assert "virtio-vga,edid=on,xres=720,yres=1440" in launch
    assert "ps2-mouse" in launch
    x86_start = server.index('if arch == "x86_64":')
    x86_end = server.index('elif arch == "aarch64":', x86_start)
    x86_profile = server[x86_start:x86_end]
    assert "virtio-vga,edid=on,xres=720,yres=1440" in x86_profile
    assert "ps2-mouse" in x86_profile
    assert "usb-tablet" not in x86_profile


def test_guest_power_is_authoritative():
    kms = read("kernel/src/drivers/display_kms.rs")
    syscall = read("kernel/src/syscall/mod.rs")
    launcher = read("apps/launcher/src/main.rs")
    assert "pub powered: bool" in kms
    assert "toggle_powered" in kms
    assert "DevicePower = 127" in syscall
    assert "sys_device_power" in syscall
    assert "std::device_power(0)" in launcher
    assert "std::device_power(1)" in launcher
    assert "std::device_power(5)" in launcher
    serial = read("kernel/src/serial.rs")
    assert 'verb.eq_ignore_ascii_case("POWER_KEY")' in serial
    assert "toggle_powered()" in serial


def test_boot_and_wake_are_guest_owned():
    launcher = read("apps/launcher/src/main.rs")
    boot = read("apps/launcher/src/screens/boot.rs")
    assert "ViewMode::BootScreen" in launcher
    assert "boot_started_ticks" in launcher
    assert "Boot presentation complete" in launcher
    assert "Display wake observed; presenting real lock screen." in launcher
    assert "get_boot_screen_color" in boot


def test_studio_pointer_events_are_serialized_in_guest_order():
    app = read("emulator/app.js")
    start = app.index("const touchSurface = screenLiveScanout || liveScanoutImg;")
    end = app.index("Draggable IDE Split Resizer Divider", start)
    pointer_code = app[start:end]
    assert "guestTouchQueue = guestTouchQueue.then" in pointer_code
    assert "sendGuestTouchEvent({ action: 'down'" in pointer_code
    assert "sendGuestTouchEvent({ action: 'move'" in pointer_code
    assert "sendGuestTouchEvent({ action: 'up'" in pointer_code
    assert "sendGuestTouchEvent({ action: 'cancel'" in pointer_code


def test_studio_clicks_have_real_pointer_down_and_up():
    app = read("emulator/app.js")
    down = app.index("action: 'down', coords: 'portrait'")
    pointer_down = app.index("touchSurface.addEventListener('pointerdown'")
    assert down > pointer_down
    assert "action: 'up', coords: 'portrait'" in app
    assert "action: 'tap', coords: 'portrait'" not in app[pointer_down:app.index("touchSurface.addEventListener('wheel'")]


def test_android_execution_and_logging_budgets_are_enforced():
    runtime = read("kernel/src/compat/android/runtime.rs")
    framework = read("kernel/src/compat/android/framework.rs")
    assert "TOTAL_INSTRUCTION_LIMIT: u64 = 1_000_000" in runtime
    assert "self.instructions_executed >= TOTAL_INSTRUCTION_LIMIT" in runtime
    assert "steps >= STEP_LIMIT || pc < insns.len()" in runtime
    assert "MAX_LOG_ENTRIES: usize = 64" in framework
    assert "MAX_LOG_LINE_BYTES: usize = 512" in framework


def test_legacy_kernel_phone_shell_is_not_canonical_boot_path():
    main = read("kernel/src/main.rs")
    assert "draw_mobile_shell()" not in main
    assert "redraw_phone()" not in main


def test_reboot_keeps_guest_scanout_visible_in_studio():
    app = read("emulator/app.js")
    assert "Rebooting InterOS..." in app
    assert "fetch('/api/qemu/reboot'" in app
    assert "if (isBooting) {" not in app[app.index("function updateDeviceDisplay"):app.index("// Exact sub-rectangle")]


def test_volume_is_guest_rendered_as_a_system_overlay():
    launcher = read("apps/launcher/src/main.rs")
    assert "render_volume_hud" in launcher
    assert "std::window_set_geometry(4, id, 100, 220, 100, true)" in launcher


def test_studio_does_not_gate_real_scanout_on_launcher_ready_state():
    app = read("emulator/app.js")
    assert "hasLauncherPresented" not in app
    assert "expose its real scanout" in app

def test_android_boundary_does_not_fabricate_framework_or_signature_success():
    vfs = read("kernel/src/compat/android/vfs.rs")
    apk = read("kernel/src/compat/android/apk.rs")
    runtime = read("kernel/src/compat/android/mod.rs")
    assert "ANDROID_APP_PROCESS_VIRTUAL_STUB" not in vfs
    assert "ANDROID_FRAMEWORK_JAR_ISOLATED_CONTAINER_RUNTIME" not in vfs
    verifier = apk[apk.index("pub fn verify_apk_signature"):apk.index("impl ApkPackage")]
    assert "Err(ApkError::InvalidSignature)" in verifier
    assert "return Ok(())" not in verifier
    assert "full Android framework/runtime compatibility is not yet implemented" in runtime

def test_volume_state_is_never_fabricated_when_guest_does_not_confirm():
    server = read("emulator/server.py")
    launcher = read("apps/launcher/src/main.rs")
    assert "cur_vol = 75" not in server
    assert "'status': 'APPLIED' if success else 'UNCONFIRMED'" in server
    assert "volume_state_valid" in launcher
    assert "unwrap_or(75)" not in launcher


def test_device_studio_power_on_and_reboot_are_not_reported_as_simulated():
    server = read("emulator/server.py")
    app = read("emulator/app.js")
    assert "monitor_alive = bool(query_qemu_monitor().get('qemu_alive', False))" in server
    assert "restart_method = 'monitor_system_reset'" in server
    assert "restart_method = 'backend_relaunch'" in server
    assert "simulated" not in app
    assert "[BOOT_TIMEOUT]" in app


def test_device_studio_never_kills_unowned_qemu_processes():
    server = read("emulator/server.py")
    stop = server[server.index("def stop_qemu_backend"):server.index("def launch_qemu_profile")]
    assert "psutil.process_iter" not in stop
    assert "p.kill()" not in stop
    assert "Only qemu_subproc, created and owned by" in stop


def test_boot_renderer_is_constant_time_and_all_view_matches_cover_boot():
    boot = read("apps/launcher/src/screens/boot.rs")
    launcher = read("apps/launcher/src/main.rs")
    assert "while distance.saturating_add(1)" not in boot
    assert "distance_sq" in boot
    assert "ViewMode::BootScreen => {" in launcher
    assert "Boot presentation complete" in launcher


def test_android_apk_without_dex_is_rejected_before_container_approval():
    apk = read("kernel/src/compat/android/apk.rs")
    audit = apk[apk.index("pub fn audit_and_create_container"):]
    assert "if !self.has_classes_dex" in audit
    assert "status: AcrAuditStatus::InvalidApkFormat" in audit
    assert "MissingDex" in apk
    assert "ok_or(ApkError::MissingDex)" in apk
    assert "if !self.has_classes_dex" in audit


def test_pointer_stream_has_single_cancel_path_and_no_duplicate_tap():
    app = read("emulator/app.js")
    pointer_start = app.index("touchSurface.addEventListener('pointerdown'")
    pointer_end = app.index("Draggable IDE Split Resizer Divider", pointer_start)
    pointer_code = app[pointer_start:pointer_end]
    assert pointer_code.count("touchSurface.addEventListener('pointercancel'") == 1
    assert "action: 'down', coords: 'portrait'" in pointer_code
    assert "action: 'move', coords: 'portrait'" in pointer_code
    assert "action: 'up', coords: 'portrait'" in pointer_code
    assert "action: 'tap', coords: 'guest_framebuffer'" not in pointer_code
    assert "action: 'swipe', coords: 'guest_framebuffer'" not in pointer_code


def test_power_on_after_guest_acpi_shutdown_uses_monitor_reset_not_serial():
    server = read("emulator/server.py")
    assert 'if "shutdown" in status_lower' in server
    assert "query_qemu_monitor(force=True)" in server
    assert "guest_status == 'shutdown'" in server
    shutdown_branch = server[server.index("guest_status == 'shutdown'"):server.index("elif vm_alive and action in ('power', 'wake', 'toggle')", server.index("guest_status == 'shutdown'"))]
    assert "send_qemu_monitor_cmd('system_reset')" in shutdown_branch
    assert "send_guest_serial_command('POWER_KEY'" not in shutdown_branch


def test_android_runtime_preserves_vm_state_for_each_active_package():
    runtime = read("kernel/src/compat/android/mod.rs")
    assert "active_vms: BTreeMap<u32, DalvikVmState>" in runtime
    assert "self.active_vms.insert(pid, DalvikVmState::new())" in runtime
    assert "self.active_vms.get_mut(&pid)" in runtime
    assert "self.active_vms.remove(&pid)" in runtime
    assert "let mut vm = DalvikVmState::new()" not in runtime



def test_studio_volume_buttons_have_one_guest_authoritative_handler():
    app = read("emulator/app.js")
    assert app.count("emuBtnVolup?.addEventListener('click'") == 1
    assert app.count("hwBtnVolup?.addEventListener('click'") == 1
    assert "volume: null" in app
    assert "state.volume = Math.max(0, Math.min(100, state.volume + delta))" not in app


def test_orientation_is_only_committed_after_guest_acknowledgement():
    server = read("emulator/server.py")
    handler = server[server.index("if self.path in ('/api/hardware/orientation', '/api/orientation'):"):server.index("if self.path == '/api/hardware/sensor_inject':")]
    assert "send_guest_serial_command(command[0], command[1], timeout=3)" in handler
    assert "status': 'UNCONFIRMED'" in handler
    assert handler.index("if not ok:") < handler.index("current_device_orientation = 'landscape'")
    assert "'status': 'APPLIED'" in handler


def test_android_dalvik_objects_are_vm_local_bounded_and_indexes_fail_closed():
    runtime = read("kernel/src/compat/android/runtime.rs")
    assert "const MAX_DALVIK_OBJECTS: usize = 256" in runtime
    assert "fn allocate_object(&mut self, class_descriptor: &str)" in runtime
    assert "self.allocate_object(class_descriptor)?" in runtime
    assert "non-null synthetic object reference" not in runtime
    assert "dex.strings.get(str_idx).ok_or(DalvikVmError::InvalidBytecode)?" in runtime
    assert "dex.types.get(type_idx).is_none()" in runtime
    assert "object_allocations_have_distinct_vm_local_identity_and_a_hard_limit" in runtime


def test_qemu_mouse_reaches_the_canonical_input_router():
    driver = read("kernel/src/drivers/ps2_mouse.rs")
    input_mod = read("kernel/src/input/mod.rs")
    interrupts = read("kernel/src/interrupts.rs")
    assert "fn decode_packet" in driver
    assert "crate::input::post_mouse_event" in driver
    assert "fn post_mouse_event" in input_mod
    assert "post_pointer_contact(InputSource::Mouse" in input_mod
    assert "crate::drivers::ps2_mouse::handle_irq_byte()" in interrupts


def test_power_menu_uses_guest_owned_shutdown_and_restart_transition():
    launcher = read("apps/launcher/src/main.rs")
    transition = read("apps/launcher/src/screens/power_transition.rs")
    assert "ViewMode::PowerTransition" in launcher
    assert "power_transition_started_ticks" in launcher
    assert "elapsed >= 60" in launcher
    assert "std::device_power(action)" in launcher
    assert "get_power_transition_color" in transition
    assert "std::device_power(0)" not in launcher[launcher.index("// Card 2: Restart"):launcher.index("// Card 3: Power off")]
    assert "std::device_power(1)" not in launcher[launcher.index("// Card 3: Power off"):launcher.index("// Card 4: Emergency")]


def test_android_invoke_uses_real_register_frames_and_full_35c_register_list():
    runtime = read("kernel/src/compat/android/runtime.rs")
    assert "fn decode_invoke_registers" in runtime
    assert "caller_registers = self.registers" in runtime
    assert "caller_strings = self.string_registers.clone()" in runtime
    assert "self.registers[base + i] = caller_registers[arg_registers[i]]" in runtime
    assert "invoke_format_35c_decodes_all_register_slots_and_rejects_bad_counts" in runtime


def test_power_transition_uses_bounded_dirty_band_for_animation_frames():
    launcher = read("apps/launcher/src/main.rs")
    transition = launcher[launcher.index("if state.current_view == ViewMode::PowerTransition"):launcher.index("if state.current_view == ViewMode::BootScreen")]
    assert "render_band(surface, &state, H / 2 - 130, (H / 2 + 220).min(H))" in transition
    assert "render_band(surface, &state, 0, H)" not in transition


def test_power_transition_is_long_enough_for_a_visible_guest_animation():
    launcher = read("apps/launcher/src/main.rs")
    assert "const POWER_TRANSITION_TICKS: u64 = 220" in launcher
    assert "elapsed.min(POWER_TRANSITION_TICKS)" in launcher
    assert "if elapsed >= POWER_TRANSITION_TICKS" in launcher


def test_android_log_dispatch_prefers_message_argument_over_tag():
    framework = read("kernel/src/compat/android/framework.rs")
    assert "string_args.get(1).and_then(|value| *value)" in framework
