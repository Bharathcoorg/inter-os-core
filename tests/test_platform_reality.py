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
        main = self.read("kernel/src/main.rs")
        platform = self.read("kernel/src/platform.rs")
        self.assertIn("pub mod platform;", main)
        self.assertIn("platform::init();", main)
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
        self.assertIn("for service_id in 1u8..=11u8", init)
        self.assertIn("registry_has_all_running_services_after_query()", init)
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
        self.assertIn("self.current_freq_mhz = self.min_freq_mhz", power)

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

if __name__ == "__main__":
    unittest.main(verbosity=2)
