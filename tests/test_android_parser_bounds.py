# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Parser-bounds security contract (H6, H17, M15, M26, M25/M27, OOM policy).

Pins the *formulas* that bound attacker-controlled amplification in the
Ring-0 Android compatibility parsers, the .rapp authenticate-before-parse
order, the FDT property slice bound, the SDHCI W1C / LBA / FIFO fixes, and
the kernel alloc_error_handler policy. Then executes the real Rust parsers
through `cargo test bounds_` so the pins are backed by running code, not
just source text.

Verification level: host unit tests only. No QEMU, no device, no hardware
claims are made by this suite.
"""

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _src(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def test_apk_zip_bomb_budget_is_a_formula_not_a_declared_size_allocation():
    source = _src("kernel/src/compat/android/apk.rs")
    # The legacy contract pin stays: MAX_MEMBER_BYTES is unchanged.
    assert "MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024" in source
    # The real output budget is min(8 MiB, compressed * 100 + 64 KiB).
    assert "MAX_INFLATED_MEMBER_BYTES: usize = 8 * 1024 * 1024" in source
    assert "MAX_MEMBER_COMPRESSION_RATIO: usize = 100" in source
    assert "MEMBER_RATIO_SLACK_BYTES: usize = 64 * 1024" in source
    assert "pub fn member_output_limit(compressed_len: usize) -> usize" in source
    assert "checked_mul(MAX_MEMBER_COMPRESSION_RATIO)" in source
    # Refusal names the measured quantity and the budget.
    assert "MemberTooLarge { bytes: usize, limit: usize }" in source
    # No allocation may be sized by the declared (attacker) u32 length.
    assert "vec![0u8; declared" not in source
    assert "Vec::with_capacity(declared" not in source
    # Peak allocation is observable for the bounded-peak assertion.
    assert "pub fn member_buffer_high_water() -> usize" in source


def test_dex_amplification_is_bounded_by_input_length():
    source = _src("kernel/src/compat/android/dex.rs")
    assert "MIN_ENCODED_METHOD_BYTES: usize = 3" in source
    assert "MAX_STRING_BYTES_FACTOR: usize = 32" in source
    # Record cap and String budget are derived from the actual input length.
    assert "raw.len() / MIN_ENCODED_METHOD_BYTES" in source
    assert ".checked_mul(MAX_STRING_BYTES_FACTOR)" in source
    assert "TooManyMethodRecords" in source
    assert "StringBudgetExceeded" in source
    # Shared class_data_off is decoded once, not once per class_def.
    assert "seen_class_data" in source
    # Index deltas and the instruction counter use checked arithmetic.
    assert "last_method_idx.checked_add(diff)" in source
    assert "last_vmethod_idx.checked_add(diff)" in source
    assert "InstructionCountOverflow" in source
    assert "m.instruction_count.checked_mul(2)" in source


def test_rapp_authenticates_before_parsing_attacker_structure():
    source = _src("kernel/src/package/rapp.rs")
    start = source.index("pub fn parse_rapp")
    end = source.index("\n}\n", start)
    body = source[start:end]
    digest_at = body.index("DigestMismatch")
    revoke_at = body.index("REVOCATION_REGISTRY")
    sign_at = body.index("verify_rapp_signature(")
    trust_at = body.index("is_key_trusted")
    caps_at = body.index("UnknownCapability")
    section_at = body.index("parse_v4_section")
    # Auth order preserved: digest -> revocation -> signature -> trust,
    # and every auth step precedes capability/section structure parsing.
    assert digest_at < revoke_at < sign_at < trust_at < caps_at
    assert trust_at < section_at
    # Canonical message arithmetic is checked, and the public verifier
    # re-validates ranges instead of trusting its caller.
    assert "payload_offset.checked_add(payload_len)" in source
    assert "RappError::PayloadOverflow" in source
    verifier = source[source.index("pub fn verify_rapp_signature"):]
    verifier = verifier[: verifier.index("\n}\n")]
    assert "checked_add" in verifier
    assert "canonical_signed_message(data,payload_offset,payload_len)?" in verifier


def test_fdt_property_slice_is_bounded_before_indexing():
    source = _src("kernel/src/drivers/fdt.rs")
    assert ".checked_add(len)" in source
    assert "prop_end > self.totalsize" in source
    assert "&self.data[offset..prop_end]" in source
    # The unbounded slice must be gone.
    assert "&self.data[offset..offset + len]" not in source
    # String reads are bounded by totalsize with checked offset arithmetic.
    assert "self.off_strings.checked_add(offset)" in source
    assert "self.data[abs_off..self.totalsize]" in source


def test_sdhci_w1c_loopback_lba_and_fifo_fixes():
    source = _src("kernel/src/drivers/sdhci.rs")
    # W1C: the clear helper writes exactly the bits being acknowledged.
    assert "fn clear_interrupts(&self, bits: u32)" in source
    assert "mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, bits);" in source
    # The inverted read-modify-write statements must be gone (comments may
    # still describe the old bug; pin the executable form only).
    assert "mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, cur & !INT_CMD_COMPLETE)" not in source
    assert "mmio_write32(self.base_vaddr, SDHCI_INT_STATUS, cur & !INT_TRANSFER_COMPLETE)" not in source
    assert "(cur & !INT_ERROR) | INT_CMD_COMPLETE" not in source
    # Completion synthesis is gated to the explicit loopback rig flag.
    assert "pub loopback: bool" in source
    assert "if self.loopback {" in source
    assert "loopback: false," in source
    # LBA: range refusal before MMIO and checked byte-address multiply.
    assert "lba >= self.total_sectors" in source
    assert "lba.checked_mul(SECTOR_SIZE as u32)" in source
    assert "else { lba * 512 }" not in source
    # FIFO loops are unaligned-safe on byte buffers.
    assert "core::ptr::write_unaligned(buf.as_mut_ptr().add(i * 4) as *mut u32, dword)" in source
    assert "core::ptr::read_unaligned(buf.as_ptr().add(i * 4) as *const u32)" in source


def test_kernel_oom_handler_is_defined_and_panic_safe():
    source = _src("kernel/src/lib.rs")
    assert "#[alloc_error_handler]" in source
    assert 'target_os = "none"' in source
    assert "fn interos_alloc_error(layout: core::alloc::Layout) -> !" in source
    start = source.index("#[alloc_error_handler]")
    handler = source[start:]
    # No panic recursion, no blocking lock, no allocation in the OOM path.
    assert "panic!" not in handler
    assert ".lock()" not in handler
    assert "hlt()" in handler
    # The serial path it uses is try_lock-based (bounded, non-blocking).
    # The only .lock() inside _print_internal is the cfg(test) capture sink,
    # which is compiled out of the bare-metal kernel entirely.
    serial = _src("kernel/src/serial.rs")
    print_start = serial.index("pub fn _print_internal")
    print_body = serial[print_start: serial.index("\n}\n", print_start)]
    assert "SERIAL1.try_lock()" in print_body
    assert "SERIAL1.lock()" not in print_body
    assert "#[cfg(test)]" in print_body


def test_real_parsers_pass_the_bounds_suite():
    """Execute the actual Rust parsers against the adversarial fixtures."""
    env = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    cmd = [
        "cargo", "test", "--manifest-path", "kernel/Cargo.toml",
        "bounds_", "--", "--nocapture",
    ]
    try:
        proc = subprocess.run(
            cmd, cwd=str(ROOT), env=env, capture_output=True, text=True, timeout=600
        )
    except subprocess.TimeoutExpired:
        raise AssertionError("cargo test bounds_ timed out after 600s")
    out = proc.stdout + proc.stderr
    assert proc.returncode == 0, (
        f"cargo test bounds_ exited {proc.returncode}\n{out[-8000:]}"
    )
    expected = [
        "apk_bounds_zip_bomb_declared_huge_refused_with_bounded_peak",
        "apk_bounds_zip_bomb_lying_declared_refused_at_ratio_budget",
        "apk_bounds_legitimate_deflate_member_still_extracts",
        "dex_bounds_shared_class_data_amplification_is_bounded",
        "dex_bounds_string_clone_budget_refuses_by_number",
        "dex_bounds_method_index_diff_overflow_refused",
        "dex_bounds_instruction_counter_overflow_checked",
        "rapp_bounds_bad_signature_refused_before_section_parse",
        "rapp_bounds_authentic_container_still_gets_structure_validated",
        "rapp_bounds_signature_range_overflow_refused_without_panic",
        "fdt_bounds_oversized_property_refused_without_slice_panic",
        "fdt_bounds_property_len_just_past_totalsize_refused",
        "fdt_bounds_forged_nameoff_and_unterminated_strings_do_not_panic",
        "fdt_bounds_valid_fixture_still_discovers_both_devices",
        "sdhci_bounds_w1c_clear_writes_only_intended_bits",
        "sdhci_bounds_lba_byte_address_overflow_refused_before_mmio",
        "sdhci_bounds_lba_past_known_capacity_refused_before_mmio",
        "sdhci_bounds_no_loopback_never_fabricates_cmd_complete",
        "sdhci_bounds_unaligned_buffer_fifo_roundtrip",
    ]
    for name in expected:
        assert name in out, f"bounds test {name} did not run\n{out[-8000:]}"
    assert "test result: ok." in out
    # Measured amplification / bounded-peak evidence must be emitted.
    assert "[APK-BOUNDS]" in out
    assert "[DEX-BOUNDS]" in out
    assert "[FDT-BOUNDS]" in out


if __name__ == "__main__":
    failures = 0
    for fn_name, fn in sorted(globals().items()):
        if fn_name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"PASS {fn_name}")
            except AssertionError as exc:
                failures += 1
                print(f"FAIL {fn_name}: {exc}")
    sys.exit(1 if failures else 0)