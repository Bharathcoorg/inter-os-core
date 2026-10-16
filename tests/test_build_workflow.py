import os
import struct
import subprocess
import sys
import tempfile
import time
import unittest

ROOT_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


def synthetic_elf_with_sections(section_names):
    """Build a minimal 64-bit ET_EXEC whose section table holds the named sections.

    Only what Assert-NoSymbolTable reads is populated: e_ident, e_type, e_shoff,
    e_shentsize, e_shnum, e_shstrndx, the section name table and sh_name per
    section. Every header field is written little-endian so the bytes match what
    the PowerShell reader expects on this platform.
    """
    shstrtab = b"\x00" + b"".join(n.encode("ascii") + b"\x00" for n in section_names)
    name_offsets = {}
    cursor = 1
    for n in section_names:
        name_offsets[n] = cursor
        cursor += len(n) + 1

    # Index 0 is the mandatory null section; index 1 is the name table itself.
    sections = [("", 0, 0), (".shstrtab", 64, len(shstrtab))]
    for n in section_names:
        if n == ".shstrtab":
            continue
        sections.append((n, 0, 0))

    e_shoff = 64 + len(shstrtab)
    e_shnum = len(sections)
    e_shstrndx = 1

    header = bytearray(64)
    header[0:4] = b"\x7fELF"
    header[4] = 2          # ELFCLASS64
    header[5] = 1          # ELFDATA2LSB
    header[6] = 1          # EV_CURRENT
    struct.pack_into("<H", header, 16, 2)      # e_type = ET_EXEC
    struct.pack_into("<Q", header, 40, e_shoff)
    struct.pack_into("<H", header, 58, 64)     # e_shentsize
    struct.pack_into("<H", header, 60, e_shnum)
    struct.pack_into("<H", header, 62, e_shstrndx)

    table = bytearray()
    for name, off, size in sections:
        entry = bytearray(64)
        struct.pack_into("<I", entry, 0, name_offsets.get(name, 0))
        struct.pack_into("<Q", entry, 24, off)   # sh_offset
        struct.pack_into("<Q", entry, 32, size)  # sh_size
        table += entry

    return bytes(header) + shstrtab + bytes(table)


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

    def test_symbol_table_assertion_rejects_elf_carrying_symtab(self):
        """Proves a stripped ELF that still carries .symtab is rejected, not accepted.

        .symtab/.strtab hold LLVM internal-linkage names of the form
        "<name>.llvm.<hash>" where the hash comes from the absolute source path.
        Leaving them in makes the same revision produce different bytes in every
        directory, so the build must fail closed rather than silently ship them.
        """
        blob = synthetic_elf_with_sections([".shstrtab", ".text", ".symtab", ".strtab"])
        with tempfile.NamedTemporaryFile("wb", prefix="elf_symtab_", delete=False) as f:
            f.write(blob)
            temp_path = f.name
        try:
            r = self.run_ps(f"Assert-NoSymbolTable -Path '{temp_path}'")
            self.assertNotEqual(r.returncode, 0, "an ELF carrying .symtab must be rejected")
            self.assertIn(".symtab", r.stderr)
            self.assertIn("different image", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_symbol_table_assertion_rejects_elf_carrying_only_strtab(self):
        """Proves a .strtab alone is enough to fail: the name table carries the hashed symbols."""
        blob = synthetic_elf_with_sections([".shstrtab", ".text", ".strtab"])
        with tempfile.NamedTemporaryFile("wb", prefix="elf_strtab_", delete=False) as f:
            f.write(blob)
            temp_path = f.name
        try:
            r = self.run_ps(f"Assert-NoSymbolTable -Path '{temp_path}'")
            self.assertNotEqual(r.returncode, 0, "an ELF carrying .strtab must be rejected")
            self.assertIn(".strtab", r.stderr)
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_symbol_table_assertion_accepts_elf_without_symbols(self):
        """Proves the assertion accepts a properly stripped ELF, so it is not a blanket failure."""
        blob = synthetic_elf_with_sections([".shstrtab", ".text", ".rodata", ".data", ".bss"])
        with tempfile.NamedTemporaryFile("wb", prefix="elf_nosym_", delete=False) as f:
            f.write(blob)
            temp_path = f.name
        try:
            r = self.run_ps(f"Assert-NoSymbolTable -Path '{temp_path}'")
            self.assertEqual(r.returncode, 0, f"a stripped ELF must be accepted: {r.stderr}\n{r.stdout}")
        finally:
            if os.path.exists(temp_path):
                os.remove(temp_path)

    def test_symbol_table_assertion_rejects_non_elf_and_truncated_input(self):
        """Proves the reader fails closed on junk instead of walking off the buffer."""
        with tempfile.NamedTemporaryFile("wb", prefix="elf_junk_", delete=False) as f:
            f.write(b"NOT_AN_ELF_AT_ALL_XXXX" * 8)
            junk = f.name
        with tempfile.NamedTemporaryFile("wb", prefix="elf_short_", delete=False) as f:
            f.write(b"\x7fELF\x02\x01\x01\x00")
            short = f.name
        try:
            r1 = self.run_ps(f"Assert-NoSymbolTable -Path '{junk}'")
            self.assertNotEqual(r1.returncode, 0)
            self.assertIn("ELF magic", r1.stderr)
            r2 = self.run_ps(f"Assert-NoSymbolTable -Path '{short}'")
            self.assertNotEqual(r2.returncode, 0)
            self.assertIn("too short", r2.stderr)
        finally:
            for p in (junk, short):
                if os.path.exists(p):
                    os.remove(p)

    def test_userspace_build_strips_symbols_and_asserts_the_result(self):
        """Proves build_userspace.ps1 both strips the table and verifies it was removed.

        This is the wiring check: the behavioural tests above prove the assertion
        works, and this proves the build actually relies on it, so removing
        --strip-unneeded can no longer pass unnoticed. The flag is checked on the
        llvm-strip invocation itself rather than anywhere in the file, because the
        explanatory comment also names the flag.
        """
        script = os.path.join(ROOT_DIR, "tools", "build_userspace.ps1")
        with open(script, "r", encoding="utf-8-sig") as f:
            build_userspace = f.read()

        strip_calls = [ln for ln in build_userspace.splitlines()
                       if "-Exe $StripTool" in ln and "Invoke-Native" in ln]
        self.assertEqual(len(strip_calls), 1,
                         f"expected exactly one llvm-strip invocation, found {len(strip_calls)}")
        self.assertIn("--strip-unneeded", strip_calls[0],
                      "the llvm-strip invocation must strip the symbol table that breaks reproducibility")
        self.assertIn("--strip-debug", strip_calls[0],
                      "the llvm-strip invocation must keep removing DWARF from the shipped image")
        self.assertIn("Assert-NoSymbolTable", build_userspace,
                      "build_userspace.ps1 must assert the symbol table is gone, not assume it")
        assert_calls = [ln for ln in build_userspace.splitlines()
                        if ln.strip().startswith("Assert-NoSymbolTable -Path $targetPath")]
        self.assertEqual(len(assert_calls), 1,
                         f"the strip must be followed by exactly one assertion on the artifact, found {len(assert_calls)}")
        strip_index = build_userspace.index(strip_calls[0])
        assert_index = build_userspace.index(assert_calls[0])
        self.assertGreater(assert_index, strip_index,
                           "the assertion must run after llvm-strip, or it checks the unstripped file")

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

    @staticmethod
    def _synthetic_kernel(payload_len=4096, marker=0x42):
        """Builds a minimal but structurally valid 64-bit x86_64 ET_DYN ELF.

        One PT_LOAD segment covers the whole file, which is the shape
        `Get-ElfLoadSegmentGeometry` and `Test-BiosImageEmbedsKernel` read.
        Built here rather than reusing a build artifact so this test needs no
        cargo build and no prebuilt image on disk.
        """
        import struct

        payload = bytearray([marker]) * payload_len
        # Pad the file so e_phoff(0x40) + phnum*phentsize leaves room for payload.
        header_size = 0x40 + 56
        total = header_size + len(payload)
        blob = bytearray(b"\x00" * total)

        blob[0:4] = b"\x7fELF"
        blob[4] = 2          # EI_CLASS: 64-bit
        blob[5] = 1          # EI_DATA: little-endian
        blob[6] = 1          # EI_VERSION
        struct.pack_into("<H", blob, 16, 3)       # e_type = ET_DYN
        struct.pack_into("<H", blob, 18, 0x3E)    # e_machine = x86_64
        struct.pack_into("<I", blob, 20, 1)       # e_version
        struct.pack_into("<Q", blob, 32, 0x40)    # e_phoff
        struct.pack_into("<H", blob, 52, 64)      # e_ehsize
        struct.pack_into("<H", blob, 54, 56)      # e_phentsize
        struct.pack_into("<H", blob, 56, 1)       # e_phnum

        ph = 0x40
        struct.pack_into("<I", blob, ph + 0, 1)          # PT_LOAD
        struct.pack_into("<Q", blob, ph + 8, 0)           # p_offset
        struct.pack_into("<Q", blob, ph + 16, 0x100000)   # p_vaddr
        struct.pack_into("<Q", blob, ph + 24, 0x100000)   # p_paddr
        struct.pack_into("<Q", blob, ph + 32, total)      # p_filesz
        struct.pack_into("<Q", blob, ph + 40, total)      # p_memsz
        struct.pack_into("<Q", blob, ph + 48, 0x1000)     # p_align

        blob[header_size:total] = payload
        return bytes(blob)

    @classmethod
    def _synthetic_image(cls, kernel_bytes, kernel_offset=1536, filler=0x11):
        """Builds a BIOS-shaped raw image with a kernel ELF at a sector offset."""
        mbr = bytearray(512)
        mbr[510] = 0x55
        mbr[511] = 0xAA
        assert kernel_offset % 512 == 0, "the bootloader writes the payload sector aligned"
        image = bytearray(filler * max(kernel_offset + len(kernel_bytes) + 512, 4096))
        image[0:512] = mbr
        image[kernel_offset:kernel_offset + len(kernel_bytes)] = kernel_bytes
        return bytes(image)

    def test_bios_image_must_embed_the_intended_kernel(self):
        """Proves the content check accepts an image holding the intended kernel."""
        kernel = self._synthetic_kernel()
        image = self._synthetic_image(kernel)
        with tempfile.NamedTemporaryFile("wb", prefix="embed_ok_", delete=False) as kf:
            kf.write(kernel)
            kernel_path = kf.name
        with tempfile.NamedTemporaryFile("wb", prefix="embed_img_", delete=False) as bf:
            bf.write(image)
            bios_path = bf.name
        try:
            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertEqual(r.returncode, 0, f"expected acceptance: {r.stderr}\n{r.stdout}")
            self.assertIn("embeds the intended kernel", r.stdout)
        finally:
            for p in (kernel_path, bios_path):
                if os.path.exists(p):
                    os.remove(p)

    def test_bios_image_rejects_a_substituted_kernel_of_the_same_size(self):
        """A same-size, same-architecture, newer-timestamped different kernel must be rejected.

        This is the case the size and modification-time proxies cannot catch: the
        substitute kernel has identical geometry and the image is larger and
        newer, so only comparing the embedded loadable bytes rejects it.
        """
        kernel = self._synthetic_kernel(marker=0x42)
        image = self._synthetic_image(kernel)

        # Same total length, same architecture, same program header geometry;
        # only the payload bytes differ, well past the header and phdr table.
        substitute = bytearray(kernel)
        substitute[0x200] ^= 0xFF
        self.assertEqual(len(substitute), len(kernel))
        self.assertNotEqual(bytes(substitute), kernel)

        with tempfile.NamedTemporaryFile("wb", prefix="subs_kernel_", delete=False) as kf:
            kf.write(bytes(substitute))
            kernel_path = kf.name
        with tempfile.NamedTemporaryFile("wb", prefix="subs_img_", delete=False) as bf:
            bf.write(image)
            bios_path = bf.name
        try:
            # Prove the proxies would have been satisfied, so the rejection below
            # is attributable to the content check alone.
            self.assertGreater(len(image), len(substitute), "image must be larger than the kernel")
            newer = time.time() - 60
            os.utime(kernel_path, (newer, newer))
            self.assertGreaterEqual(os.path.getmtime(bios_path), os.path.getmtime(kernel_path))

            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0, "a substituted kernel must be rejected")
            self.assertIn("no sector-aligned ELF", r.stderr)
            self.assertIn("different kernel", r.stderr)
        finally:
            for p in (kernel_path, bios_path):
                if os.path.exists(p):
                    os.remove(p)

    def test_bios_image_rejects_an_image_with_no_kernel_payload(self):
        """An image that is larger and newer but holds no ELF payload must be rejected."""
        with tempfile.NamedTemporaryFile("wb", prefix="noelf_img_", delete=False) as bf:
            sector = bytearray(16384)
            sector[510] = 0x55
            sector[511] = 0xAA
            bf.write(bytes(sector))
            bios_path = bf.name
        kernel = self._synthetic_kernel()
        with tempfile.NamedTemporaryFile("wb", prefix="noelf_kbin_", delete=False) as kf:
            kf.write(kernel)
            kernel_path = kf.name
        try:
            # Make the image unambiguously newer so the cheap staleness guard
            # passes and the content check is what has to reject this.
            os.utime(kernel_path, (time.time() - 60, time.time() - 60))
            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("no sector-aligned ELF", r.stderr)
        finally:
            for p in (kernel_path, bios_path):
                if os.path.exists(p):
                    os.remove(p)

    def test_bios_image_content_check_rejects_a_non_elf_kernel_reference(self):
        """A reference that is not a 64-bit little-endian ELF must fail closed."""
        with tempfile.NamedTemporaryFile("wb", prefix="badref_img_", delete=False) as bf:
            sector = bytearray(8192)
            sector[510] = 0x55
            sector[511] = 0xAA
            bf.write(bytes(sector))
            bios_path = bf.name
        with tempfile.NamedTemporaryFile("wb", prefix="badref_kbin_", delete=False) as kf:
            kf.write(b"\x00" * 4096)
            kernel_path = kf.name
        try:
            os.utime(kernel_path, (time.time() - 60, time.time() - 60))
            ps = f"Verify-BiosImageArtifact -ImagePath '{bios_path}' -KernelBinaryPath '{kernel_path}'"
            r = self.run_ps(ps)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("ELF magic", r.stderr)
        finally:
            for p in (kernel_path, bios_path):
                if os.path.exists(p):
                    os.remove(p)

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
            with open(path, "r", encoding="utf-8-sig") as f:
                content = f.read()
            self.assertNotIn("LastWriteTimeUtc -ge $buildStartTime", content, f"{rel_path} has stale mtime check")
            self.assertNotIn(".LastWriteTime -ge (Get-Item", content, f"{rel_path} has stale mtime check")
            self.assertNotIn(".LastWriteTime -gt (Get-Item", content, f"{rel_path} has stale mtime check")

        # WHY these positive assertions are here and were restored: without them
        # the checks above only prove the old pattern is absent, which a file
        # that had deleted its verification entirely would also satisfy.
        with open(os.path.join(ROOT_DIR, "dev.ps1"), "r", encoding="utf-8-sig") as f:
            dev = f.read()
        self.assertIn("Invoke-CargoBuildArtifact", dev, "dev.ps1 must build through the structured artifact helper")
        self.assertIn("Verify-BiosImageArtifact", dev, "dev.ps1 must verify the BIOS image")
        self.assertIn("-KernelBinaryPath", dev, "dev.ps1 must pass the kernel binary to the BIOS image verification")

        with open(os.path.join(ROOT_DIR, "qemu", "build.ps1"), "r", encoding="utf-8-sig") as f:
            qemu_build = f.read()
        self.assertIn("Invoke-CargoBuildArtifact", qemu_build, "qemu/build.ps1 must build through the structured artifact helper")
        self.assertIn("Verify-BiosImageArtifact", qemu_build, "qemu/build.ps1 must verify the BIOS image")
        for target in ("x86_64-unknown-none", "aarch64-unknown-none", "riscv64gc-unknown-none-elf"):
            self.assertIn(target, qemu_build, f"qemu/build.ps1 must build the {target} target explicitly")

        with open(os.path.join(ROOT_DIR, "tests", "test_aarch64.ps1"), "r", encoding="utf-8-sig") as f:
            aarch64_test = f.read()
        self.assertIn("Verify-KernelBinaryArtifact", aarch64_test, "test_aarch64.ps1 must verify the AArch64 binary")

        # The BIOS check must be a content check, not a size and timestamp proxy.
        with open(os.path.join(ROOT_DIR, "tools", "build_env.ps1"), "r", encoding="utf-8-sig") as f:
            build_env = f.read()
        self.assertIn("Test-BiosImageEmbedsKernel", build_env,
                      "build_env.ps1 must verify the image embeds the intended kernel")
        self.assertIn("Get-ElfLoadSegmentGeometry", build_env,
                      "build_env.ps1 must locate the kernel by its ELF load extents")

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
