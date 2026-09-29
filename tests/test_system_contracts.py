#!/usr/bin/env python3
"""Static contract checks for core reliability/developer surfaces."""

from pathlib import Path
import json
import unittest
import re

ROOT = Path(__file__).resolve().parents[1]

class SystemContractTests(unittest.TestCase):
    def test_studio_guest_authority_has_no_host_vfs_shadow(self):
        server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        self.assertNotIn("VFS_ROOT", server)
        self.assertNotIn("resolve_vfs_path", server)
        self.assertIn('source": "GUEST_VFS"', server)
        self.assertIn('source": "GUEST_REGISTRY"', server)
        self.assertIn("def is_safe_guest_path", server)
        self.assertIn("def is_safe_package_id", server)
        self.assertIn("guest command rejected by transport bounds", server)

    def test_launcher_uses_guest_catalog_and_does_not_render_fixed_demo_telemetry(self):
        launcher = (ROOT / "apps/launcher/src/main.rs").read_text(encoding="utf-8")
        self.assertIn("package_registry", launcher)
        self.assertIn("app_runtime_list", launcher)
        for literal in (
            "24*C", "Cloudy - Humidity 64%", "82%", "Compiling kernel... 12m left",
            "Interstellar", "Hans Zimmer - Soundtrack", "11:00 AM", "3,482",
            "steps today / 10k goal", "RAM: 18/64 MB | InterFS: 42/128 GB | 2.4 GHz",
        ):
            self.assertNotIn(literal, launcher)

    def test_sdk_application_session_exposes_supported_platform_services(self):
        app = (ROOT / "sdk/rust/interos-std/src/app.rs").read_text(encoding="utf-8")
        for method in (
            "send_ipc", "receive_ipc", "post_notification", "cancel_notification",
            "open_socket", "wait_for_events",
        ):
            self.assertIn(f"pub fn {method}", app)

    def test_developer_cli_exposes_reproducible_core_workflow(self):
        cli = (ROOT / "tools/interos.py").read_text(encoding="utf-8")
        for command in (
            'subparsers.add_parser("doctor"',
            'subparsers.add_parser("build"',
            'subparsers.add_parser("test"',
            'subparsers.add_parser("contracts"',
            'subparsers.add_parser("package"',
            'subparsers.add_parser("verify"',
            'subparsers.add_parser("run"',
            'subparsers.add_parser("inspect"',
            'subparsers.add_parser("install"',
            'subparsers.add_parser("uninstall"',
        ):
            self.assertIn(command, cli)
        self.assertIn("target_lower in (\"demo\", \"qemu\", \"phone\")", cli)

    def test_sdk_and_kernel_syscall_tables_match(self):
        kernel = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
        pattern = re.compile(r"^\\s*([A-Za-z][A-Za-z0-9_]*)\\s*=\\s*(\\d+),", re.MULTILINE)
        kernel_ids = dict(pattern.findall(kernel.split("pub enum SyscallNumber", 1)[1]))
        sdk_ids = dict(pattern.findall(sdk.split("pub enum SyscallNumber", 1)[1]))
        self.assertEqual(kernel_ids, sdk_ids)

    def test_reliability_modules_exist(self):
        for rel in (
            "kernel/src/recovery.rs",
            "kernel/src/update.rs",
            "kernel/src/diagnostics.rs",
            "kernel/src/performance.rs",
            "kernel/src/watchdog.rs",
        ):
            self.assertTrue((ROOT / rel).is_file(), rel)

    def test_kernel_registers_reliability_subsystems(self):
        main = (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8")
        for name in ("recovery", "update", "diagnostics", "performance", "watchdog"):
            self.assertIn(f"pub mod {name};", main)

    def test_qemu_phone_userspace_manifest_matches_kernel_image_registry(self):
        manifest_path = ROOT / "platform/x86_64/userspace-manifest.json"
        self.assertTrue(manifest_path.is_file())
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        names = [item["name"] for item in manifest["required"]]
        images = (ROOT / "kernel/src/task/images.rs").read_text(encoding="utf-8")
        for name in names:
            self.assertIn(f"/{name}", images, f"{name} is not embedded by the kernel image registry")
        self.assertEqual(len(names), len(set(names)))

    def test_qemu_phone_contract_uses_one_vnc_endpoint(self):
        contract = (ROOT / "docs/QEMU_PHONE_PLATFORM.md").read_text(encoding="utf-8")
        studio = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        self.assertIn("127.0.0.1:5901", contract)
        self.assertIn('"-vnc", "127.0.0.1:1"', studio)
        self.assertIn("127.0.0.1:5901", studio)

    def test_build_cli_exposes_canonical_demo(self):
        cli = (ROOT / "tools/interos.py").read_text(encoding="utf-8")
        self.assertIn('target_lower in ("demo", "qemu", "phone")', cli)
        self.assertIn('p_run.add_argument("name", help="demo/qemu or app name")', cli)
        self.assertIn("_build_x86_userspace()", cli)

    def test_qemu_userspace_manifest_sources_exist(self):
        manifest = json.loads((ROOT / "platform/x86_64/userspace-manifest.json").read_text(encoding="utf-8"))
        for item in manifest["required"]:
            self.assertTrue((ROOT / item["manifest"]).is_file(), item["manifest"])

    def test_guest_rapp_lifecycle_transport_is_wired(self):
        serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
        studio = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        store = (ROOT / "kernel/src/package/store.rs").read_text(encoding="utf-8")
        syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
        for command in ("RAPP_BEGIN", "RAPP_CHUNK", "RAPP_COMMIT", "RAPP_TRUST", "RAPP_LAUNCH", "RAPP_UNINSTALL"):
            self.assertIn(command, serial)
        for route in ("/api/rapp/install", "/api/rapp/trust", "/api/rapp/launch", "/api/rapp/uninstall"):
            self.assertIn(route, studio)
        self.assertIn("pub fn put(&mut self,package_id:&str", store)
        self.assertIn("PACKAGE_BLOB_STORE.lock().get(app_id)", syscall)
        self.assertIn("pub fn app_launch", sdk)
        self.assertIn("parse_rapp(&state.bytes)", serial)
        self.assertIn("stage_verified", serial)
        self.assertIn("activate(&verified.package_id)", serial)

    def test_qemu_phone_input_contract_is_canonical(self):
        contract = json.loads((ROOT / "platform/x86_64/qemu-phone.json").read_text(encoding="utf-8"))
        server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
        app = (ROOT / "emulator/app.js").read_text(encoding="utf-8")
        self.assertEqual(contract["display"]["width"], 1280)
        self.assertEqual(contract["display"]["height"], 720)
        self.assertEqual(contract["input"]["coordinate_space"], "display-pixels")
        self.assertIn("INPUT_TOUCH", serial)
        self.assertIn("INPUT_KEY", serial)
        self.assertIn("INPUT_TOUCH DOWN", server)
        self.assertIn("/api/input/key", server)
        self.assertIn("INPUT_KEY", server)
        self.assertIn("codePointAt(0)", app)
        self.assertIn("Math.min(1279", app)
        self.assertIn("/api/touch/event", app)
        self.assertIn("/api/input/key", app)
        self.assertIn("0 <= x < 1280 and 0 <= y < 720", server)
        self.assertIn("scanout_width: 1280", (ROOT / "kernel/src/drivers/display_kms.rs").read_text(encoding="utf-8"))
        self.assertIn("scanout_height: 720", (ROOT / "kernel/src/drivers/display_kms.rs").read_text(encoding="utf-8"))

    def test_studio_does_not_fabricate_serial_command_results(self):
        server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        serial_route = server[server.index("if self.path == '/api/serial/write':"):server.index("if self.path == '/api/themes/apply':")]
        self.assertNotIn('uname -a', serial_route)
        self.assertNotIn('ps / tasks', serial_route)
        self.assertIn("UART_COM1", serial_route)

    def test_dev_launch_has_explicit_qemu_boundary(self):
        syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
        self.assertIn("pub fn dev_launch_verified_rapp", syscall)
        self.assertIn("VerificationLevel::QemuVerified", syscall)
        self.assertIn("dev_launch_verified_rapp(id)", serial)

    def test_syscall_pointer_safety_is_enforced(self):
        user_ptr = (ROOT / "kernel/src/syscall/user_ptr.rs").read_text(encoding="utf-8")
        memory = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
        syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        self.assertIn("validate_current_user_buffer", user_ptr)
        self.assertIn("validate_user_range(addr, len, write)", user_ptr)
        self.assertIn("PermissionDenied", user_ptr)
        self.assertIn("Misaligned", user_ptr)
        self.assertIn("copy_to_kernel_typed", user_ptr)
        self.assertIn("user frame is not owned by task", memory)
        self.assertIn("flags.contains(PageTableFlags::USER_ACCESSIBLE)", memory)
        self.assertIn("flags.contains(PageTableFlags::WRITABLE)", memory)
        self.assertNotIn("core::slice::from_raw_parts(a4 as *const u32", syscall)
        self.assertNotIn("from_raw_parts_mut", syscall)
        self.assertNotIn("core::ptr::copy_nonoverlapping", syscall)

    def test_arch_syscalls_do_not_directly_dereference_unvalidated_pointers(self):
        aarch = (ROOT / "kernel-aarch64/src/syscall.rs").read_text(encoding="utf-8")
        riscv = (ROOT / "kernel-riscv64/src/syscall.rs").read_text(encoding="utf-8")
        self.assertIn("fn validate_user_range", aarch)
        self.assertIn("fn copy_from_user", aarch)
        self.assertIn("fn copy_to_user", aarch)
        self.assertIn("fn validate_user_range", riscv)
        self.assertIn("fn copy_from_user", riscv)
        self.assertNotIn("core::slice::from_raw_parts", aarch)
        self.assertNotIn("core::slice::from_raw_parts", riscv)
        self.assertIn("checked_add", aarch)
        self.assertIn("checked_add", riscv)

    def test_service_memory_budget_is_enforced_before_scheduler_registration(self):
        user = (ROOT / "kernel/src/task/user.rs").read_text(encoding="utf-8")
        services = (ROOT / "kernel/src/services.rs").read_text(encoding="utf-8")
        self.assertIn("spawn_user_process_with_limits", user)
        self.assertIn("resource_limits(service)", user)
        self.assertIn("address_space_bytes as u64 > limits.max_address_space_bytes", user)
        self.assertIn("set_limits(process, resource_limits(id))", services)

    def test_package_install_is_a_real_transaction(self):
        syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        self.assertIn("stage_verified(&verified)", syscall)
        self.assertIn("PACKAGE_BLOB_STORE", syscall)
        self.assertIn("activate_version_with_policy(pkg_id, verified.version, &verified.policy)", syscall)
        self.assertIn("store.remove(pkg_id)", syscall)
        self.assertIn("PACKAGE_REGISTRY.lock().rollback(pkg_id)", syscall)

    def test_package_blob_restore_fails_closed(self):
        store = (ROOT / "kernel/src/package/store.rs").read_text(encoding="utf-8")
        self.assertIn("let n=b[4] as usize", store)
        self.assertIn("if n>MAX_BLOBS { return; }", store)
        self.assertIn("if p!=len { return; }", store)
        self.assertIn("self.blobs=restored;", store)

    def test_update_is_fail_closed_and_monotonic(self):
        src = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
        self.assertIn("manifest.version <= current_version", src)
        self.assertIn("DigestMismatch", src)
        self.assertIn("InvalidSignature", src)
        self.assertIn("recovery_required", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)
