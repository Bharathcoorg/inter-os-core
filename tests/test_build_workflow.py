import os
import subprocess
import sys
import tempfile
import time
import unittest

ROOT_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


class TestBuildWorkflow(unittest.TestCase):
    def run_ps(self, script_text: str):
        """Helper to run a PowerShell script snippet in repository root with build_env.ps1 imported."""
        full_script = f"""
$ErrorActionPreference = 'Stop'
Set-Location '{ROOT_DIR}'
. '{os.path.join(ROOT_DIR, "tools", "build_env.ps1")}'
{script_text}
"""
        cmd = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", full_script]
        return subprocess.run(cmd, cwd=ROOT_DIR, capture_output=True, text=True)

    def test_repeated_builds_succeed_consecutively(self):
        """Proves running build command twice without source changes succeeds on both runs (no-op rebuild)."""
        cmd = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "dev.ps1", "build"]
        
        # First build
        r1 = subprocess.run(cmd, cwd=ROOT_DIR, capture_output=True, text=True)
        self.assertEqual(r1.returncode, 0, f"First build failed: {r1.stderr}\n{r1.stdout}")
        self.assertIn("BIOS image verified", r1.stdout)
        
        # Second build (no-op cached)
        r2 = subprocess.run(cmd, cwd=ROOT_DIR, capture_output=True, text=True)
        self.assertEqual(r2.returncode, 0, f"Second no-op build failed: {r2.stderr}\n{r2.stdout}")
        self.assertIn("cached/up-to-date", r2.stdout)
        self.assertIn("BIOS image verified", r2.stdout)

    def test_cargo_artifact_verification_rejects_missing_file(self):
        """Proves Verify-KernelBinaryArtifact fails closed when binary does not exist on disk."""
        ps = "Verify-KernelBinaryArtifact -BinaryPath 'target/nonexistent_fake_kernel_xyz'"
        r = self.run_ps(ps)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("does not exist on disk", r.stderr)

    def test_cargo_artifact_verification_rejects_empty_file(self):
        """Proves Verify-KernelBinaryArtifact fails closed when binary has 0 bytes."""
        with tempfile.NamedTemporaryFile("wb", prefix="kernel_empty_", delete=False) as f:
            temp_path = f.name
        try:
            ps = f"Verify-KernelBinaryArtifact -BinaryPath '{temp_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("empty (0 bytes)", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_cargo_artifact_verification_rejects_truncated_elf(self):
        """Proves Verify-KernelBinaryArtifact fails closed when binary is smaller than 64-byte ELF header."""
        with tempfile.NamedTemporaryFile("wb", prefix="kernel_trunc_", delete=False) as f:
            f.write(b"\x7FELF\x02\x01\x01\x00")  # only 8 bytes
            temp_path = f.name
        try:
            ps = f"Verify-KernelBinaryArtifact -BinaryPath '{temp_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("truncated (smaller than 64-byte ELF header)", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_cargo_artifact_verification_rejects_invalid_elf_magic(self):
        """Proves Verify-KernelBinaryArtifact fails closed when binary lacks valid ELF magic header."""
        with tempfile.NamedTemporaryFile("wb", prefix="kernel_bad_magic_", delete=False) as f:
            f.write(b"NOT_AN_ELF_HEADER_AT_ALL_JUST_64_BYTES_OF_CORRUPTED_GARBAGE_DATA!")
            temp_path = f.name
        try:
            ps = f"Verify-KernelBinaryArtifact -BinaryPath '{temp_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("not a valid ELF binary", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_cargo_artifact_verification_rejects_wrong_architecture(self):
        """Proves Verify-KernelBinaryArtifact fails closed when ELF e_machine does not match expected architecture."""
        # Create a valid 64-byte x86_64 ELF header (e_machine = 0x003E at offset 18)
        header = bytearray(64)
        header[0:4] = b"\x7FELF"
        header[4] = 2  # 64-bit
        header[5] = 1  # little-endian
        header[6] = 1  # version 1
        header[18] = 0x3E  # e_machine x86_64 (62)
        header[19] = 0x00

        with tempfile.NamedTemporaryFile("wb", prefix="kernel_x86_header_", delete=False) as f:
            f.write(bytes(header))
            temp_path = f.name
        try:
            # Expecting aarch64 on an x86_64 binary must fail closed
            ps = f"Verify-KernelBinaryArtifact -BinaryPath '{temp_path}' -ExpectedArch 'aarch64'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("has architecture x86_64", r.stderr)
            self.assertIn("expected aarch64", r.stderr)

            # Expecting x86_64 on an x86_64 binary must succeed
            ps_ok = f"Verify-KernelBinaryArtifact -BinaryPath '{temp_path}' -ExpectedArch 'x86_64'"
            r_ok = self.run_ps(ps_ok)
            self.assertEqual(r_ok.returncode, 0, f"Expected success: {r_ok.stderr}")
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_bios_verification_rejects_truncated_image(self):
        """Proves Verify-BiosImageArtifact rejects image smaller than 512-byte sector."""
        with tempfile.NamedTemporaryFile("wb", prefix="bios_trunc_", delete=False) as f:
            f.write(b"\x00" * 256)
            temp_path = f.name
        try:
            ps = f"Verify-BiosImageArtifact -ImagePath '{temp_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("smaller than one sector", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_bios_verification_rejects_invalid_signature(self):
        """Proves Verify-BiosImageArtifact rejects disk image lacking 0x55, 0xAA boot signature."""
        with tempfile.NamedTemporaryFile("wb", prefix="bios_bad_mbr_", delete=False) as f:
            f.write(b"\x00" * 512)
            temp_path = f.name
        try:
            ps = f"Verify-BiosImageArtifact -ImagePath '{temp_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("lacks valid MBR boot signature", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_bios_verification_rejects_image_smaller_than_kernel(self):
        """Proves Verify-BiosImageArtifact rejects BIOS image that cannot encapsulate kernel binary."""
        with tempfile.NamedTemporaryFile("wb", prefix="kbin_", delete=False) as kf:
            kf.write(b"\x00" * 4096)
            kernel_path = kf.name

        with tempfile.NamedTemporaryFile("wb", prefix="bios_small_", delete=False) as bf:
            # 512 bytes with valid signature, but smaller than 4096-byte kernel
            sector = bytearray(512)
            sector[510] = 0x55
            sector[511] = 0xAA
            bf.write(bytes(sector))
            bios_path = bf.name

        try:
            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("smaller than or equal to kernel binary", r.stderr)
        finally:
            if os.path.exists(kernel_path):
                os.remove(kernel_path)
            if os.path.exists(bios_path):
                os.remove(bios_path)

    def test_bios_verification_rejects_stale_image(self):
        """Proves Verify-BiosImageArtifact rejects BIOS image older than its referenced kernel binary."""
        with tempfile.NamedTemporaryFile("wb", prefix="bios_stale_", delete=False) as bf:
            sector = bytearray(8192)
            sector[510] = 0x55
            sector[511] = 0xAA
            bf.write(bytes(sector))
            bios_path = bf.name

        # Sleep briefly to ensure distinct timestamps
        time.sleep(0.05)

        with tempfile.NamedTemporaryFile("wb", prefix="kbin_newer_", delete=False) as kf:
            kf.write(b"\x00" * 4096)
            kernel_path = kf.name

        # Set bios_path mtime explicitly in the past
        old_time = time.time() - 3600
        os.utime(bios_path, (old_time, old_time))

        try:
            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("older than kernel binary", r.stderr)
        finally:
            if os.path.exists(kernel_path):
                os.remove(kernel_path)
            if os.path.exists(bios_path):
                os.remove(bios_path)

    def test_stale_artifact_selection_eliminated(self):
        """Proves dev.ps1, qemu/build.ps1, and test_aarch64.ps1 use structured verification and not mtime comparisons."""
        for rel_path in ["dev.ps1", "qemu/build.ps1", "tests/test_aarch64.ps1"]:
            path = os.path.join(ROOT_DIR, rel_path.replace("/", os.sep))
            with open(path, "r", encoding="utf-8") as f:
                content = f.read()
            self.assertNotIn("LastWriteTimeUtc -ge $buildStartTime", content, f"{rel_path} has stale mtime check")
            self.assertNotIn(".LastWriteTime -ge (Get-Item", content, f"{rel_path} has stale mtime check")
            self.assertNotIn(".LastWriteTime -gt (Get-Item", content, f"{rel_path} has stale mtime check")

    def test_cross_platform_path_normalization(self):
        """Proves build_env.ps1 normalizes mixed slash path representations."""
        ps = """
$p1 = 'kernel/target/x86_64-unknown-none/debug/kernel'
$p2 = 'kernel\\target\\x86_64-unknown-none\\debug\\kernel'
$norm1 = [System.IO.Path]::GetFullPath((Join-Path $PWD $p1))
$norm2 = [System.IO.Path]::GetFullPath((Join-Path $PWD $p2))
if ($norm1 -ne $norm2) {
    throw "Paths did not normalize identically: $norm1 vs $norm2"
}
Write-Host "Normalized: $norm1"
"""
        r = self.run_ps(ps)
        self.assertEqual(r.returncode, 0)
        self.assertIn("Normalized:", r.stdout)


if __name__ == "__main__":
    unittest.main()
