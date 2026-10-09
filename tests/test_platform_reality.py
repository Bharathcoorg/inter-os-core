#!/usr/bin/env python3
"""Static platform-reality contracts for the InterOS Core repository."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PlatformRealityTests(unittest.TestCase):
    def read(self, path):
        return (ROOT / path).read_text(encoding="utf-8")

    def test_platform_contract_is_bootstrapped(self):
        # Module declarations live in the library; the binary owns only the panic
        # handler and the boot sequence.
        lib = self.read("kernel/src/lib.rs")
        boot = self.read("kernel/src/main.rs")
        platform = self.read("kernel/src/platform.rs")
        self.assertIn("pub mod platform;", lib)
        self.assertIn("kernel::platform::init();", boot)
        self.assertIn("HardwareAvailability::Emulated", platform)
        self.assertIn("VerificationLevel::QemuVerified", platform)

    def test_kernel_does_not_mark_qemu_as_physical_hardware(self):
        platform = self.read("kernel/src/platform.rs")
        self.assertIn("pub const fn is_hardware_backed", platform)
        self.assertIn("HardwareAvailability::Present", platform)
        self.assertNotIn("hardware: HardwareAvailability::Present", platform)

    def test_launcher_does_not_present_fixed_hardware_telemetry_as_truth(self):
        launcher = self.read("apps/launcher/src/main.rs")
        forbidden = [
            'b"09:20"',
            'b"Mon, 12 Jan"',
            'b"Bangalore"',
            "78% fill",
        ]
        for value in forbidden:
            self.assertNotIn(value, launcher)

    def test_existing_truthful_hardware_defaults_remain_in_place(self):
        wireless = self.read("kernel/src/net/wireless.rs")
        battery = self.read("kernel/src/resource/battery.rs")
        pmic = self.read("kernel/src/drivers/pmic.rs")
        camera = self.read("kernel/src/hal/camera.rs")
        self.assertIn("hardware_present: false", wireless)
        self.assertIn("telemetry_valid: false", battery)
        self.assertIn("hardware_present: false", pmic)
        self.assertIn("sensor_model: \"unavailable\"", camera)

    def test_pmic_self_test_exercises_hardware_backed_path(self):
        pmic = self.read("kernel/src/drivers/pmic.rs")
        self.assertIn("pmic.set_telemetry(", pmic)
        self.assertIn("assert!(pmic.is_hardware_present());", pmic)
        self.assertIn("assert_eq!(pmic.coulomb_counter_mah, 3150.0);", pmic)
        self.assertIn("assert_eq!(pmic.telemetry.soc_percent, 78);", pmic)

    def test_service_start_revalidates_parent_ownership(self):
        services = self.read("kernel/src/services.rs")
        self.assertIn("if task.parent_id != parent_id {", services)
        self.assertIn('return Err("running service is not owned by caller");', services)

    def test_pid1_parses_public_service_snapshot_contract(self):
        init = self.read("userspace/init/src/main.rs")
        self.assertIn("let Some(abi_bytes) = fields.next() else", init)
        self.assertIn('if abi_bytes != b"1" || state != b"running"', init)
        self.assertNotIn('task_bytes == b"0"', init)

    def test_pid1_bootstrap_retries_dependency_blocked_services(self):
        init = self.read("userspace/init/src/main.rs")
        self.assertIn("start_declared_services(&service_buf[..len])", init)
        self.assertIn("registry_has_all_running_services", init)
        self.assertIn("if !progress", init)

    def test_pid1_lifecycle_smoke_uses_dependency_leaf(self):
        init = self.read("userspace/init/src/main.rs")
        self.assertIn("std::service_stop(11)", init)
        self.assertNotIn("std::service_stop(1)", init)

    def test_battery_syscall_reports_backend_availability(self):
        syscall = self.read("kernel/src/syscall/mod.rs")
        self.assertIn('"telemetry_valid"', syscall)
        self.assertIn("pmic.is_hardware_present()", syscall)
        self.assertNotIn('r#"{{"present":true,"voltage_mv"', syscall)

    def test_modem_defaults_to_hardware_absent(self):
        modem = self.read("kernel/src/telephony/modem.rs")
        syscall = self.read("kernel/src/syscall/mod.rs")
        self.assertIn("hardware_present: bool", modem)
        self.assertIn("hardware_present: false", modem)
        self.assertIn("let is_present = modem.hardware_present;", syscall)
        self.assertNotIn("modem.state != crate::telephony::ModemRadioState::PowerDown", syscall)

    def test_raw_keystore_keys_never_cross_userspace_boundary(self):
        syscall = self.read("kernel/src/syscall/mod.rs")
        self.assertIn("Raw keystore-derived key material must never cross", syscall)
        self.assertIn("Err(SyscallResult::PermissionDenied)", syscall)

    def test_aarch64_storage_does_not_fabricate_command_completion(self):
        storage = self.read("kernel-aarch64/src/storage.rs")
        self.assertIn("hardware_present: bool", storage)
        self.assertIn("hardware unavailable", storage.lower())

        # The status register is hardware-owned and write-1-to-clear. Setting a
        # completion bit would manufacture command completion; acknowledging one
        # by clearing it is the only legitimate write.
        writes = re.findall(
            r"mmio_write\d+\(\s*self\.base_vaddr,\s*SDHCI_INT_STATUS,\s*(?P<value>[^;]*)\)",
            storage,
        )
        self.assertTrue(writes, "no SDHCI status-register writes found to audit")
        for value in writes:
            self.assertIn("&", value)
            self.assertNotIn("|", value, f"SDHCI status write sets a bit: {value}")
        self.assertNotIn("(cur & !INT_ERROR) | INT_CMD_COMPLETE", storage)
        self.assertNotIn("SDHCI_INT_STATUS, INT_CMD_COMPLETE", storage)

    def test_aarch64_power_requires_real_telemetry(self):
        power = self.read("kernel-aarch64/src/power.rs")
        self.assertIn("hardware_present: bool", power)
        self.assertIn("if !self.hardware_present || !battery.hardware_present", power)
        # The applied clock is only meaningful once a controller has reported its
        # operating points; before that the governor must record "no clock applied"
        # rather than a plausible default frequency.
        self.assertIn("NO_FREQ_MHZ", power)
        self.assertIn("current_freq_mhz: u32", power)

    def test_aarch64_runtime_selftests_do_not_fabricate_sd_hardware(self):
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertIn("let sdhci = SdhciController::new(0);", main)
        self.assertIn("SDHCI hardware unavailable", main)
        self.assertNotIn("mmio_sdhci_space", main)
        self.assertNotIn("INT_CMD_COMPLETE", main)

    def test_aarch64_runtime_selftests_do_not_fabricate_power_or_audio(self):
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertIn("assert!(!battery.hardware_present)", main)
        self.assertIn("assert!(!governor.hardware_present)", main)
        self.assertIn("assert!(!audio_driver.hardware_present)", main)
        self.assertNotIn("Simulated DMA Hardware Pump", main)

    def test_riscv_does_not_enable_sum_globally_on_trap_init(self):
        trap = self.read("kernel-riscv64/src/trap.rs")
        init = trap[trap.index("pub unsafe fn init_traps()"):trap.index("#[unsafe(naked)]")]
        self.assertNotIn("1usize << 18", init)
        # Trap init must actively clear SUM, not merely avoid setting it: firmware
        # leaves supervisor user-memory access enabled.
        self.assertIn("csrrc zero, sstatus, {2}", init)
        # The enable/disable pair is scoped to one audited helper rather than being
        # duplicated inline wherever user memory is copied.
        mmu = self.read("kernel-riscv64/src/mmu.rs")
        syscall = self.read("kernel-riscv64/src/syscall.rs")
        self.assertIn("csrrs zero, sstatus", mmu)
        self.assertIn("csrrc zero, sstatus", mmu)
        self.assertIn("pub unsafe fn with_user_memory", mmu)
        self.assertNotIn("csrrs zero, sstatus", syscall)

    def test_aarch64_camera_never_returns_synthetic_frames(self):
        camera = self.read("kernel-aarch64/src/camera.rs")
        self.assertIn("hardware_present: bool", camera)
        self.assertIn("if !self.hardware_present || !self.enabled", camera)
        self.assertNotIn("Generate representative test pattern", camera)

    def test_riscv_unsupported_ipc_does_not_report_success(self):
        syscall = self.read("kernel-riscv64/src/syscall.rs")
        self.assertIn("5 => -38", syscall)
        self.assertIn("6 => -38", syscall)
        self.assertIn("SCHEDULER.lock().current_task_id()", syscall)

    def test_riscv_trap_epilogue_preserves_kernel_stack(self):
        trap = self.read("kernel-riscv64/src/trap.rs")
        self.assertIn("csrw sscratch, sp", trap)
        self.assertIn("mv sp, t0", trap)
        self.assertNotIn('csrrw sp, sscratch, sp",\n        "sret"', trap)

    def test_riscv_faults_and_exit_update_scheduler_state(self):
        task = self.read("kernel-riscv64/src/task.rs")
        trap = self.read("kernel-riscv64/src/trap.rs")
        main = self.read("kernel-riscv64/src/main.rs")
        self.assertIn("pub fn exit_current(&mut self) -> bool", task)
        self.assertIn("task.state = TaskState::Exited", task)
        self.assertIn("SCHEDULER.lock().exit_current()", trap)
        self.assertIn("SCHEDULER.lock().add_task", main)

    def test_aarch64_smp_requires_real_psci_gic_backend(self):
        smp = self.read("kernel-aarch64/src/smp.rs")
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertIn("hardware_present: bool", smp)
        self.assertIn("if !self.hardware_present", smp)
        self.assertIn("SmpError::HardwareDisabled", smp)
        self.assertIn("assert!(!smp.hardware_present)", main)
        self.assertNotIn("smp.boot_secondary_core(1, 0x4008_0000, 0, &smp_caps).expect", main)
        self.assertNotIn("smp.dispatch_ipi(0b0000_0010, smp::IpiKind::Reschedule, &smp_caps).expect", main)

    def test_aarch64_mobile_hal_backends_default_to_absent(self):
        modules = {
            "kernel-aarch64/src/gps.rs": "hardware_present: bool",
            "kernel-aarch64/src/bluetooth.rs": "hardware_present: bool",
            "kernel-aarch64/src/sensors.rs": "hardware_present: bool",
            "kernel-aarch64/src/nfc.rs": "hardware_present: bool",
            "kernel-aarch64/src/audio.rs": "hardware_present: bool",
            "kernel-aarch64/src/usb.rs": "hardware_present: bool",
            "kernel-aarch64/src/haptics.rs": "hardware_present: bool",
            "kernel-aarch64/src/biometrics.rs": "hardware_present: bool",
            "kernel-aarch64/src/strongbox.rs": "hardware_present: bool",
            "kernel-aarch64/src/killswitch.rs": "hardware_present: bool",
            "kernel-aarch64/src/sim.rs": "hardware_present: bool",
            "kernel-aarch64/src/display_hal.rs": "hardware_present: bool",
        }
        for path, field in modules.items():
            source = self.read(path)
            self.assertIn(field, source)
            self.assertIn("hardware_present: false", source)

    def test_aarch64_killswitch_uses_real_ed25519_verification(self):
        killswitch = self.read("kernel-aarch64/src/killswitch.rs")
        self.assertIn("verify_ed25519_signature", killswitch)
        self.assertNotIn("expected_tag[i] ^ self.owner_pubkey[i]", killswitch)

    def test_aarch64_wifi_does_not_claim_sae_without_hardware_backend(self):
        wifi = self.read("kernel-aarch64/src/wifi.rs")
        self.assertIn("hardware_present: bool", wifi)
        self.assertIn("HardwareUnavailable", wifi)
        self.assertIn("Err(WifiError::HardwareUnavailable)", wifi)
        self.assertNotIn("let ephemeral_rand = [0x55u8; 32]", wifi)

    def test_aarch64_ota_uses_asymmetric_publisher_signatures(self):
        ota = self.read("kernel-aarch64/src/ota.rs")
        crypto = self.read("kernel-aarch64/src/crypto.rs")
        cargo = self.read("kernel-aarch64/Cargo.toml")
        self.assertIn("pub signature: [u8; 64]", ota)
        self.assertIn("verify_ed25519_signature", ota)
        self.assertIn("ed25519_dalek", crypto)
        self.assertIn("ed25519-dalek", cargo)
        self.assertNotIn("HmacSha256::mac(&self.publisher_pubkey", ota)

    def test_keystore_does_not_claim_hardware_backing_without_backend(self):
        keystore = self.read("kernel/src/crypto/keystore.rs")
        self.assertIn("hardware_backed: bool", keystore)
        self.assertIn("hardware_backed: false", keystore)
        self.assertIn("is_hardware_backed", keystore)
        self.assertIn("software-backed keystore", keystore)

    def test_aarch64_pci_is_discovered_from_the_device_tree(self):
        bus = self.read("kernel-aarch64/src/pci_bus.rs")
        ecam = self.read("kernel-aarch64/src/pcie.rs")
        mmu = self.read("kernel-aarch64/src/mmu.rs")
        main = self.read("kernel-aarch64/src/main.rs")
        # The bus must come from the device tree, never from a fixed address.
        self.assertIn("find_node_by_compatible(\"pci-host-ecam-generic\")", bus)
        self.assertIn('get_node_property(&node, "reg")', bus)
        self.assertIn("pub const fn unavailable() -> Self", bus)
        # ECAM config space addressing is 1 MiB per bus, 32 KiB per device slot and
        # 4 KiB per function. The legacy 0xCF8 windows are 256 bytes and read
        # the wrong register everywhere.
        self.assertIn("ECAM_BUS_STRIDE: usize = 0x10_0000", ecam)
        self.assertIn("ECAM_FUNCTION_STRIDE: usize = 0x1000", ecam)
        self.assertIn("(((device as usize) * 8) + (function as usize)) * ECAM_FUNCTION_STRIDE", ecam)
        # QEMU places the ECAM window at 256 GiB + 256 MiB, which is 2 MiB but
        # not 1 GiB aligned, so it needs its own level 2 table.
        self.assertIn("pub fn map_device_region_high", mmu)
        self.assertIn("L2_HIGH_TABLE", mmu)
        # A level 1 table pointer means each leaf descriptor must carry the full
        # physical address. Writing the offset within the gigabyte instead points
        # every leaf at physical zero, so the window reads back as empty.
        self.assertIn("(start_paddr + step * L2_BLOCK)", mmu)
        self.assertNotIn("let base = start_paddr - l1_index * L1_BLOCK;", mmu)
        # Enumeration must report rather than assume.
        self.assertIn("enumerate_bus0", ecam)
        self.assertIn("PcieBus::init_from_fdt", main)

    def test_aarch64_boot_asserts_fail_closed_not_fabricated_hardware(self):
        main = self.read("kernel-aarch64/src/main.rs")
        # No panel is bound and no fuel gauge is attached, so these must not
        # assert a real refresh rate or an enabled controller.
        self.assertIn("governor.display_target_fps, power::NO_REFRESH_TARGET_HZ", main)
        self.assertIn("governor.current_freq_mhz, power::NO_FREQ_MHZ", main)
        self.assertIn("assert!(!display_hal.enabled);", main)
        self.assertIn("assert!(!display_hal.hardware_present);", main)
        self.assertNotIn("assert_eq!(governor.display_target_fps, 30);", main)
        self.assertNotIn("assert!(display_hal.enabled);", main)

    def test_writes_are_durable_before_they_report_success(self):
        syscall = self.read("kernel/src/syscall/mod.rs")
        vfs = self.read("kernel/src/fs/vfs.rs")
        sdk = self.read("sdk/rust/interos-std/src/lib.rs")

        # A write lands in an in-memory snapshot; only sync() copies it out and
        # issues the ATA CACHE FLUSH. Nothing else on the write path called it,
        # so a successful write existed only in RAM and was lost on reboot.
        write = syscall[syscall.index("pub fn sys_fs_write("):]
        write = write[: write.index("\npub fn ") if "\npub fn " in write else len(write)]
        self.assertIn("vfs.sync()", write)
        self.assertIn("written > 0", write)
        # A failed flush must not be reported as a successful write.
        self.assertIn("SyscallResult::DeviceError", write)

        # The durability barrier must be reachable from userspace and must verify
        # descriptor ownership before flushing on a task's behalf.
        self.assertIn("pub fn fsync_for_task", vfs)
        self.assertIn("pub fn sys_fs_fsync", syscall)
        self.assertIn("pub fn fs_fsync", sdk)
        self.assertIn("FsSync", sdk)
        # The flush must actually reach the controller.
        self.assertIn("drive.flush()", vfs)

    def test_no_hardware_backed_strings_are_painted_or_reported(self):
        framebuffer = self.read("kernel/src/framebuffer.rs")
        modem = self.read("kernel/src/telephony/modem.rs")
        # No radio is attached on this platform, so the system must not paint a
        # carrier, a link standard or a signal level it never measured.
        for fabricated in ("INTER_SOVEREIGN_5G", "RSSI: 31/31", "WPA3-Enterprise",
                           "Beaconing Active", "Sub-6GHz + mmWave"):
            self.assertNotIn(fabricated, framebuffer)
        for reported in ("no wireless hardware discovered", "no controller discovered",
                         "no modem discovered", "Signal: unavailable"):
            self.assertIn(reported, framebuffer)
        # A modem with no hardware must not report a strong signal or a live data
        # context with an address.
        self.assertIn("rssi: 99", modem)
        self.assertIn("pdp_context_active: false", modem)
        self.assertIn("ip_address: [0; 4]", modem)

    def test_persistent_journal_records_are_authenticated(self):
        journal = self.read("kernel/src/storage/persistent.rs")
        keystore = self.read("kernel/src/crypto/keystore.rs")
        # An unkeyed digest only proves the payload is unchanged, so anything able
        # to write the data disk could replace the trust root, revocation list or
        # storage seed and still validate. Records are tagged with a keyed MAC.
        self.assertIn("fn record_tag", journal)
        self.assertIn("HmacSha256::compute(&journal_auth_key()", journal)
        self.assertNotIn("Sha256::digest(payload)", journal)
        self.assertNotIn("Sha256::digest(&payload[..len])", journal)
        # The key is published by the keystore rather than read from it inside
        # the commit path, which would re-enter that lock and deadlock the boot.
        self.assertIn("static JOURNAL_AUTH_KEY", journal)
        self.assertIn("pub fn set_journal_auth_key", journal)
        self.assertIn("persistent::set_journal_auth_key(&ks.master_seed)", keystore)

    def test_vault_decryption_uses_persistent_authenticated_envelopes(self):
        vault = self.read("kernel/src/storage/vault.rs")
        syscall = self.read("kernel/src/syscall/mod.rs")
        # Vault authentication must be derived from the persisted envelope and
        # keystore-derived key. An in-memory list of previously issued tags would
        # make valid persisted data impossible to reopen after reboot.
        self.assertIn("block.verify(&self.vault_key)", vault)
        self.assertNotIn("fn record_sealed", vault)
        self.assertNotIn("fn matches_sealed", vault)
        self.assertNotIn("issued", vault)
        self.assertIn("fresh vault instance", vault)
        self.assertNotIn("block_with_tag.hmac_tag = block_with_tag.compute_hmac", syscall)
        self.assertNotIn("hmac_tag: [0u8; 32], // Computed on envelope", syscall)

    def test_refresh_policy_does_not_claim_an_unbound_panel(self):
        power = self.read("kernel/src/resource/power.rs")
        self.assertIn("display_min_refresh_hz", power)
        self.assertIn("display_max_refresh_hz", power)
        self.assertIn("set_display_refresh_range", power)
        self.assertIn("if self.display_max_refresh_hz == 0", power)
        # Policy targets must be negotiated against the advertised panel range,
        # not presented as universal 30/60/90/120 Hz hardware facts.
        self.assertIn("select_refresh_rate(120)", power)
        self.assertIn("select_refresh_rate(90)", power)
        self.assertIn("select_refresh_rate(60)", power)
        self.assertIn("select_refresh_rate(30)", power)

    def test_studio_does_not_invent_scanout_dimensions_or_apk_trust(self):
        server = self.read("emulator/server.py")
        app = self.read("emulator/app.js")
        self.assertIn('"framebuffer": {"width": None, "height": None', server)
        self.assertIn("not cryptographically verified", server)
        self.assertIn("function getGuestPointerRect()", app)
        self.assertNotIn("coords: 'landscape'", app)

    def test_guest_is_never_handed_the_boot_image_as_storage(self):
        # The kernel uses the Primary Master as the boot image and the Primary
        # Slave as persistent storage. With a single disk it falls back to
        # formatting the boot image, which overwrites the bootloader stages the
        # kernel is embedded in, so every boot after the first loads a corrupted
        # kernel. Every launcher must therefore attach a second disk.
        ata = self.read("kernel/src/drivers/ata.rs")
        self.assertIn("ATA_BOOT", ata)
        self.assertIn("ATA_PRIMARY", ata)
        self.assertIn("new_primary_slave", ata)

        server = self.read("emulator/server.py")
        self.assertIn("def ensure_data_disk(", server)
        self.assertIn('f"format=raw,file={img_path},index=0"', server)
        self.assertIn('f"format=raw,file={data_disk_path},index=1"', server)
        self.assertIn("interos-data-{arch}.img", server)

        for script in ("qemu/run.ps1",):
            text = self.read(script)
            self.assertIn("inter-os-data", text)
            self.assertIn("index=0", text)
            self.assertIn("index=1", text)

        interaction = self.read("tests/qemu/interaction_x86.py")
        self.assertIn("def ensure_data_disk(", interaction)
        self.assertIn("index=0", interaction)
        self.assertIn("index=1", interaction)

    def test_no_internal_gate_labels_enter_public_source(self):
        for path in ["README.md", "kernel/src/platform.rs", "tests/test_platform_reality.py"]:
            text = self.read(path).lower()
            self.assertNotRegex(text, r"\bgate\s+[abcd]\b")

    def test_userspace_build_script_cannot_report_a_false_success(self):
        script = self.read("tools/build_userspace.ps1")
        # Native tools write progress to stderr, so a blanket ErrorActionPreference
        # of Stop aborts every build; a blanket Continue lets a failed step fall
        # through to a success message. Failures must be judged by exit code.
        self.assertIn('$ErrorActionPreference = "Stop"', script)
        self.assertIn("$LASTEXITCODE", script)
        self.assertIn("throw", script)
        # A build step that reports success must have produced its artifact.
        self.assertIn("was not produced", script)
        # The success banner must not be reachable without the checks above.
        success = script.index("All Ring-3 userspace binaries built")
        self.assertLess(script.index("Assert-ExecutableElf -Path $targetPath"), success)
        self.assertLess(script.index("was not produced"), success)

    def test_userspace_build_produces_fixed_address_executables(self):
        script = self.read("tools/build_userspace.ps1")
        # The kernel ELF loader rejects ET_DYN, and the x86_64-unknown-none target
        # defaults to position-independent executables. Without this the build
        # produces images the guest cannot load and the device boots headless.
        self.assertIn("-no-pie", script)
        self.assertIn("ET_EXEC", script)
        self.assertIn("Assert-ExecutableElf", script)

    def test_userspace_build_covers_every_embedded_image(self):
        script = self.read("tools/build_userspace.ps1")
        # kernel/src/task/images.rs embeds these; a name that is embedded but never
        # built ships as a stale artifact with no compiler error.
        images = self.read("kernel/src/task/images.rs")
        for name in ("init", "displayd", "inputd", "storaged", "netd", "powerd",
                     "modemd", "audiod", "camerad", "sensord", "compositor", "launcher"):
            self.assertIn(name, images)
            self.assertIn(name, script, f"{name} is embedded by the kernel but not built")
        self.assertIn("kernel-embedded user images", script)

    def test_kernel_declares_its_embedded_user_images_as_build_inputs(self):
        build_rs = self.read("kernel/build.rs")
        # include_bytes! is invisible to Cargo's fingerprint, so a rebuilt app
        # could otherwise ship inside the kernel image as a stale copy.
        self.assertIn("rerun-if-changed", build_rs)
        for name in ("launcher", "compositor", "init"):
            self.assertIn(name, build_rs)

    def test_guest_touch_uses_the_logical_portrait_surface(self):
        serial = self.read("kernel/src/serial.rs")
        main = self.read("emulator/server.py")
        # INPUT_TOUCH must accept the space the compositor and apps use. The old
        # letterbox remap rejected every gesture above the landscape midpoint.
        self.assertIn("LOGICAL_WIDTH", serial)
        self.assertIn("LOGICAL_HEIGHT", serial)
        self.assertNotIn("x >= 460 && x <= 820", serial)
        self.assertNotIn("460 + (x * 360 // 720)", main)
        # Legacy verbs had no bounds check at all.
        self.assertIn("touch_point", serial)
        self.assertIn("portrait_from_framebuffer", main)

    def test_aarch64_boot_path_has_no_orphan_code_after_panic_handler(self):
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertEqual(main.count("#[panic_handler]"), 1)
        handler = main.index("#[panic_handler]")
        for marker in ("// 40.", "// 41.", "// 42.", "// 43.", "// 44.", "let mut smp =", "let mut sim ="):
            self.assertNotIn(
                marker,
                main[handler:],
                f"{marker!r} appears after the panic handler as orphan module-scope code",
            )
        # The boot self-test sections must live inside aarch64_kernel_main.
        self.assertLess(main.index("// 40."), main.index("#[panic_handler]"))

    def test_aarch64_sources_contain_no_escaped_newline_artifacts(self):
        # A literal "\n" written into source collapses a block onto one line and
        # silently unbalances delimiters, so the crate stops compiling.
        for path in ("kernel-aarch64/src/nfc.rs", "kernel-aarch64/src/main.rs"):
            source = self.read(path)
            self.assertNotRegex(
                source,
                r"[^\\]\\n\s{4,}\S",
                f"{path} contains a literal escaped newline mid-statement",
            )

    def test_aarch64_ndef_record_header_is_bounds_checked(self):
        nfc = self.read("kernel-aarch64/src/nfc.rs")
        parser = nfc[nfc.index("pub fn parse_ndef"):]
        # The three-octet record header must be validated before it is indexed.
        header_index = parser.index("payload[offset + 1]")
        self.assertIn("offset + 3 > payload.len()", parser[:header_index])
        # Record payload extents must be derived from the remaining length.
        self.assertIn("type_len + payload_len > payload.len() - offset", parser)

    def test_aarch64_ota_selftest_signs_with_ed25519(self):
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertIn("crypto::sign_ed25519_signature(&publisher_seed", main)
        self.assertIn("crypto::ed25519_public_key(&publisher_seed)", main)
        # An HMAC tag is a symmetric 32-byte value and must never stand in for
        # the asymmetric publisher signature the OTA engine requires.
        self.assertNotIn("HmacSha256::mac(&publisher", main)
        self.assertNotIn("signature: [0u8; 32]", main)
        self.assertIn("signature: [0u8; 64]", main)

    def test_aarch64_touch_fails_closed_without_a_digitizer(self):
        touch = self.read("kernel-aarch64/src/touch.rs")
        self.assertIn("pub hardware_present: bool", touch)
        self.assertIn("hardware_present: false", touch)
        self.assertIn("if !self.hardware_present", touch)
        self.assertIn('return Err("GT911: digitizer hardware unavailable")', touch)
        self.assertIn("pub fn set_hardware_present", touch)

    def test_aarch64_telephony_fails_closed_without_a_modem(self):
        telephony = self.read("kernel-aarch64/src/telephony.rs")
        self.assertIn("pub hardware_present: bool", telephony)
        self.assertIn("hardware_present: false", telephony)
        # Neither modem byte-stream ingestion nor signal quality may succeed
        # while no baseband is bound.
        self.assertIn("pub fn ingest_modem_rx", telephony)
        self.assertIn("pub fn read_signal_quality", telephony)
        self.assertIn('return Err("SerialRIL: modem hardware unavailable")', telephony)

    def test_aarch64_interrupt_controller_comes_from_the_device_tree(self):
        gic = self.read("kernel-aarch64/src/gic.rs")
        main = self.read("kernel-aarch64/src/main.rs")
        # The distributor and CPU interface must come from the platform, not a
        # constant, and an unrecognised model must not be programmed as GICv2.
        self.assertIn("pub fn describe_interrupt_controller", gic)
        self.assertIn("find_node_by_property(\"interrupt-controller\")", gic)
        self.assertIn("IntcDescription::Unsupported", gic)
        self.assertIn("arm,gic-v2m", gic)
        self.assertIn("describe_interrupt_controller(parser)", main)
        self.assertNotIn("Gicv2::new(DEFAULT_GICD_BASE, DEFAULT_GICC_BASE)", main)
        # GICv2m and GICv3 are not register-compatible and must not be claimed.
        self.assertNotIn("\"[OK] GICv2: Distributor @", main)

    def test_aarch64_framebuffer_geometry_comes_from_the_device_tree(self):
        display = self.read("kernel-aarch64/src/display.rs")
        main = self.read("kernel-aarch64/src/main.rs")
        self.assertIn("pub fn from_fdt", display)
        self.assertIn("SimpleFramebuffer::from_fdt(p, \"framebuffer\", 0)", main)
        # A hardcoded scanout base would claim a panel that was never discovered.
        self.assertNotIn("0x5000_0000, // Physical scanout base", main)

    def test_aarch64_arm_image_header_precedes_the_entry_code(self):
        linker = self.read("kernel-aarch64/linker.ld")
        # The arm64 boot protocol puts the first instruction at offset 0x40 and
        # the header's `code0` branches there. A separate .head output section is
        # pushed to the next segment boundary by PT_LOAD alignment, which lands
        # that branch in padding, so the header must be part of .text.
        self.assertNotIn(".head :", linker)
        self.assertIn(".text : {", linker)
        header_at = linker.index("LONG(0x644d5241)")
        text_at = linker.index(".text : {")
        self.assertLess(text_at, header_at)

    def test_fdt_validates_header_regions_before_walking(self):
        fdt = self.read("kernel-aarch64/src/fdt.rs")
        new_body = fdt[fdt.index("pub fn new("):fdt.index("pub unsafe fn from_raw_ptr")]
        # Every region the walk relies on must be proven to fit inside the blob.
        self.assertIn("struct_end > totalsize", new_body)
        self.assertIn("strings_end > totalsize", new_body)
        self.assertIn("totalsize > data.len()", new_body)
        self.assertIn("FDT_FIRST_SUPPORTED_VERSION", new_body)
        self.assertIn("FDT_LAST_SUPPORTED_VERSION", new_body)

    def test_fdt_bounds_property_values_against_the_structure_block(self):
        fdt = self.read("kernel-aarch64/src/fdt.rs")
        read_property = fdt[fdt.index("fn read_property("):]
        read_property = read_property[: read_property.index("\n    }\n")]
        # A property length is untrusted input and must be checked before slicing.
        self.assertIn("len > limit - value_start", read_property)
        self.assertIn("next > limit", read_property)
        self.assertIn("self.get_string(name_off).is_none()", read_property)
        self.assertNotIn("&self.data[value_start..value_end];\n        let", fdt)

    def test_fdt_node_matching_requires_an_exact_or_unit_address_name(self):
        fdt = self.read("kernel-aarch64/src/fdt.rs")
        matcher = fdt[fdt.index("fn node_name_matches("):]
        matcher = matcher[: matcher.index("\n    }\n")]
        # A bare prefix test can bind MMIO to the wrong node (`intc` vs `intcmux`).
        self.assertIn("rest.starts_with('@')", matcher)
        self.assertNotIn("name.starts_with(target_name)", fdt)

    def test_riscv_supervisor_user_memory_access_is_disabled(self):
        trap = self.read("kernel-riscv64/src/trap.rs")
        main = self.read("kernel-riscv64/src/main.rs")
        # SUM lets S-mode bypass page permissions; firmware leaves it set.
        self.assertIn("csrrc zero, sstatus, {2}", trap)
        self.assertIn("SSTATUS_SUM", trap)
        self.assertIn("~((1 << 8) | (1 << 18))", trap)
        self.assertIn("trap::read_sstatus() & mmu::SSTATUS_SUM", main)

    def test_riscv_ecall_advance_handles_compressed_instructions(self):
        mmu = self.read("kernel-riscv64/src/mmu.rs")
        trap = self.read("kernel-riscv64/src/trap.rs")
        self.assertIn("pub unsafe fn instruction_len", mmu)
        self.assertIn("if halfword & 0x3 != 0x3 { 2 } else { 4 }", mmu)
        self.assertIn("crate::mmu::instruction_len(tf.sepc)", trap)
        # A fixed 32-bit step would resume mid-instruction after a `c.ecall`.
        self.assertNotIn("tf.sepc += 4", trap)

    def test_aarch64_boot_log_claims_no_unmeasured_telemetry(self):
        main = self.read("kernel-aarch64/src/main.rs")
        # Signal strength, battery charge and radio state have no sensor or
        # modem backend attached at boot, so none may appear as measured data.
        for literal in ("dBm", "100% BAT", "5G NSA", "100% 5G"):
            self.assertNotIn(literal, main, f"{literal!r} is unmeasured telemetry")
        self.assertIn("no modem is bound", main)
        self.assertIn("no digitizer is attached", main)
        self.assertIn("no secure element is bound", main)

    def test_x86_page_fault_report_path_is_allocation_free_and_bounded(self):
        interrupts = self.read("kernel/src/interrupts.rs")
        handler = interrupts[interrupts.index('extern "x86-interrupt" fn page_fault_handler'):interrupts.index('extern "x86-interrupt" fn general_protection_fault_handler')]
        # The fault reporter must not recurse through formatting, locks, panic
        # formatting, or the bootloader serial path after paging has failed.
        self.assertIn("fault_report_bytes", handler)
        self.assertIn("fault_report_line", handler)
        # CR2 must be captured with a raw register read. The typed accessor
        # returns a Result that would have to be unwrapped, and a fault handler
        # must not panic. Assert the absence of any panic path rather than a
        # literal, so an explanatory comment cannot satisfy or break it.
        self.assertIn('core::arch::asm!("mov {}, cr2"', handler)
        self.assertIn("out(reg) accessed_address", handler)
        for panic_path in (".unwrap()", ".expect(", "panic!("):
            self.assertNotIn(panic_path, handler)
        self.assertIn("stack_frame.instruction_pointer.as_u64()", handler)
        self.assertIn("stack_frame.stack_pointer.as_u64()", handler)
        self.assertIn("error_code.bits()", handler)
        self.assertNotIn("serial_println!", handler)
        self.assertNotIn("panic!(", handler)
        self.assertNotIn("{:#?}", handler)
        self.assertIn("crate::task::terminate_current()", handler)
        self.assertIn("halt_after_fault_report_failure()", handler)

        # The bounds live in the shared reporting helpers every fatal handler
        # calls, so they are asserted at module scope rather than inside a single
        # handler body. A length must be clamped and a UART wait must be bounded,
        # or a corrupted frame can spin or overrun the reporting buffer.
        self.assertIn("fn fault_report_byte(", interrupts)
        self.assertIn("fn fault_report_bytes(", interrupts)
        self.assertIn("fn fault_report_line(", interrupts)
        self.assertIn("const MAX_BYTES: usize = 256", interrupts)
        self.assertIn("core::cmp::min(bytes.len(), MAX_BYTES)", interrupts)
        self.assertIn("for _ in 0..10_000", interrupts)
        self.assertIn("fn halt_after_fault_report_failure(", interrupts)

    def test_x86_fatal_exception_handlers_avoid_recursive_reporting(self):
        interrupts = self.read("kernel/src/interrupts.rs")
        for name in (
            'double_fault_handler',
            'general_protection_fault_handler',
        ):
            start = interrupts.index('extern "x86-interrupt" fn ' + name)
            end = interrupts.find('extern "x86-interrupt" fn ', start + 20)
            handler = interrupts[start:] if end == -1 else interrupts[start:end]
            self.assertIn("fault_report_bytes", handler)
            self.assertIn("fault_report_line", handler)
            self.assertIn("halt_after_fault_report_failure()", handler)
            self.assertNotIn("serial_println!", handler)
            self.assertNotIn("panic!(", handler)
            self.assertNotIn("{:#?}", handler)

    def test_mmio_mapping_is_overflow_checked_and_transactional(self):
        memory = self.read("kernel/src/memory.rs")
        start = memory.index("pub fn map_mmio_region(")
        end = memory.index("\n/// Allocates a single 4 KiB", start)
        body = memory[start:end]
        for literal in ("size.checked_add(4095)", "pages.checked_mul(4096)", "checked_add(span as u64)", "mapped_pages", "PageAlreadyMapped", "mapper.unmap(mapped)"):
            self.assertIn(literal, body)
        self.assertNotIn("size.saturating_add(4095)", body)

    def test_process_table_reports_real_cr3_values(self):
        scheduler = self.read("kernel/src/task/scheduler.rs")
        start = scheduler.index("pub fn dump_process_table")
        end = scheduler.index("\n    }", start)
        body = scheduler[start:end]
        self.assertIn("frame().start_address().as_u64()", body)
        self.assertIn("crate::memory::kernel_pml4()", body)
        self.assertNotIn("0x0000000000400000", body)
        self.assertNotIn("0x0000000000100000", body)

    def test_relocation_pointer_arithmetic_is_checked(self):
        relocate = self.read("kernel/src/arch/relocate.rs")
        for literal in ("base.checked_add", "dyn_vaddr.checked_add(dyn_memsz)", "r_offset.checked_add", "checked_add(base)"):
            self.assertIn(literal, relocate)

    def test_simplefb_rejects_missing_or_invalid_hardware_geometry(self):
        fb = self.read("kernel-aarch64/src/display.rs")
        start = fb.index("pub fn from_fdt")
        end = fb.index("pub const fn buffer_size", start)
        body = fb[start:end]
        # A display surface needs a real physical base; falling back to the
        # virtual address would present heap or identity-mapped memory as a
        # scanout.
        self.assertIn("Missing framebuffer reg property", body)
        # Geometry arrives from a bootloader-supplied device tree and must be
        # validated before it becomes an allocation or a pixel offset.
        self.assertIn("Framebuffer geometry is zero", body)
        self.assertIn("Framebuffer row size overflow", body)
        self.assertIn("Framebuffer stride is smaller than one scanline", body)
        self.assertIn("Framebuffer size overflow", body)
        self.assertIn("Framebuffer reg property has no base address", body)

    def test_fdt_parser_bounds_strings_and_memory_ranges(self):
        fdt = self.read("kernel-aarch64/src/fdt.rs")
        # Header regions are validated against totalsize before any token is read,
        # and the walk limits are proven to lie inside the blob.
        self.assertIn("let strings_end = (off_dt_strings as usize)", fdt)
        self.assertIn("checked_add(size_dt_strings as usize)", fdt)
        self.assertIn("if strings_end > totalsize", fdt)
        self.assertIn("let struct_end = (off_dt_struct as usize)", fdt)
        self.assertIn("if struct_end > totalsize", fdt)
        self.assertIn("fn struct_limit(&self) -> usize", fdt)
        self.assertIn("fn strings_limit(&self) -> usize", fdt)
        # A property length is untrusted input and is checked before slicing.
        self.assertIn("if len > limit - value_start", fdt)
        # Totals must not be wrappable by a hostile tree.
        self.assertIn("total.saturating_add(region_size)", fdt)
        # Unit-address matching must be deliberate. A bare prefix test would let
        # "intc" match "intcmux" and bind MMIO to the wrong node.
        self.assertIn("fn node_name_matches(name: &str, target: &str) -> bool", fdt)
        self.assertIn("rest.starts_with('@')", fdt)
if __name__ == "__main__":
    unittest.main(verbosity=2)
