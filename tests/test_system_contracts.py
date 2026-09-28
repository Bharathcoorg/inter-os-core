#!/usr/bin/env python3
"""Static contract checks for core reliability/developer surfaces."""

from pathlib import Path
import json
import unittest

ROOT = Path(__file__).resolve().parents[1]

class SystemContractTests(unittest.TestCase):
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
        self.assertIn('"vnc", "127.0.0.1:1"', studio)
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

    def test_phase3_qemu_input_contract_is_canonical(self):
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

    def test_phase2_dev_launch_has_explicit_qemu_boundary(self):
        syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
        serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")
        self.assertIn("pub fn dev_launch_verified_rapp", syscall)
        self.assertIn("VerificationLevel::QemuVerified", syscall)
        self.assertIn("dev_launch_verified_rapp(id)", serial)

    def test_update_is_fail_closed_and_monotonic(self):
        src = (ROOT / "kernel/src/update.rs").read_text(encoding="utf-8")
        self.assertIn("manifest.version <= current_version", src)
        self.assertIn("DigestMismatch", src)
        self.assertIn("InvalidSignature", src)
        self.assertIn("recovery_required", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)
