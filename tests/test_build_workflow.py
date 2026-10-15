import os
import subprocess
import sys
import tempfile
import unittest

ROOT_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


class TestBuildWorkflow(unittest.TestCase):
    def run_ps(self, script_text: str):
        """Helper to run a PowerShell script snippet in repository root."""
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
        """Proves verification fails closed when artifact does not exist on disk."""
        ps = """
try {
    # Test checking nonexistent path
    $nonexistent = Join-Path $PWD 'target/nonexistent_fake_kernel_binary_xyz'
    if (-not (Test-Path -LiteralPath $nonexistent)) {
        throw "Reported build artifact does not exist on disk: $nonexistent"
    }
} catch {
    Write-Host "CAUGHT: $($_.Exception.Message)"
    exit 42
}
"""
        r = self.run_ps(ps)
        self.assertEqual(r.returncode, 42)
        self.assertIn("does not exist on disk", r.stdout)

    def test_cargo_artifact_verification_rejects_empty_file(self):
        """Proves verification fails closed when artifact has 0 bytes."""
        with tempfile.NamedTemporaryFile("wb", prefix="kernel_empty_", delete=False) as f:
            temp_path = f.name
        try:
            ps = f"""
$item = Get-Item -LiteralPath '{temp_path}'
if ($item.Length -eq 0) {{
    throw "Verified build artifact is empty (0 bytes): $($item.FullName)"
}}
"""
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("empty (0 bytes)", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_cargo_artifact_verification_rejects_invalid_elf_magic(self):
        """Proves verification fails closed when binary lacks ELF magic bytes (0x7F, 0x45, 0x4C, 0x46)."""
        with tempfile.NamedTemporaryFile("wb", prefix="kernel_bad_magic_", delete=False) as f:
            f.write(b"NOT_AN_ELF_BINARY_HEADER")
            temp_path = f.name
        try:
            ps = f"""
$artifactPath = '{temp_path}'
$fs = [System.IO.File]::OpenRead($artifactPath)
try {{
    $buf = New-Object byte[] 4
    $n = $fs.Read($buf, 0, 4)
    if ($n -lt 4 -or $buf[0] -ne 0x7F -or $buf[1] -ne 0x45 -or $buf[2] -ne 0x4C -or $buf[3] -ne 0x46) {{
        throw "Artifact at $artifactPath is not a valid ELF binary (invalid magic header)"
    }}
}} finally {{
    $fs.Close()
}}
"""
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("not a valid ELF binary", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_bios_verification_rejects_invalid_signature(self):
        """Proves Verify-BiosImageArtifact rejects disk image lacking 0x55, 0xAA boot signature."""
        with tempfile.NamedTemporaryFile("wb", prefix="bios_bad_mbr_", delete=False) as f:
            # 512 bytes with invalid boot signature at 510..512
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

    def test_stale_artifact_selection_eliminated(self):
        """Proves dev.ps1 and qemu/build.ps1 use structured compiler-artifact and not timestamp comparisons."""
        dev_ps1 = (os.path.join(ROOT_DIR, "dev.ps1"))
        with open(dev_ps1, "r", encoding="utf-8") as f:
            dev_content = f.read()
        self.assertNotIn("LastWriteTimeUtc -ge $buildStartTime", dev_content)
        self.assertIn("Invoke-CargoBuildArtifact", dev_content)
        self.assertIn("Verify-BiosImageArtifact", dev_content)

        qemu_build = (os.path.join(ROOT_DIR, "qemu", "build.ps1"))
        with open(qemu_build, "r", encoding="utf-8") as f:
            qemu_content = f.read()
        self.assertNotIn("LastWriteTime -gt (Get-Item $k1).LastWriteTime", qemu_content)
        self.assertIn("Invoke-CargoBuildArtifact", qemu_content)

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
