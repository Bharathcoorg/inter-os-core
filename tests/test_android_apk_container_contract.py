# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_apk_archive_parser_rejects_partial_or_ambiguous_directory():
    source = (ROOT / "kernel/src/compat/android/apk.rs").read_text(encoding="utf-8")
    assert "MAX_ENTRIES: usize = 4096" in source
    assert "MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024" in source
    assert "if cd_end != eocd_pos || cd_end > data.len()" in source
    assert "if cursor != cd_end || entries.len() != total_entries" in source
    assert "name.split('/').any(|part| part == \"..\")" in source
    assert "if entries[..i].iter().any(|entry| entry.name == entries[i].name)" in source


def test_android_apk_signature_verification_does_not_accept_unverified_signatures():
    source = (ROOT / "kernel/src/compat/android/apk.rs").read_text(encoding="utf-8")
    start = source.index("pub fn verify_apk_signature")
    end = source.index("\n}\n", start)
    verifier = source[start:end]
    assert "Err(ApkError::InvalidSignature)" in verifier
    assert "Ok(())" not in verifier


def test_dalvik_runtime_remains_bounded_and_fails_closed_for_unsupported_framework_calls():
    source = (ROOT / "kernel/src/compat/android/runtime.rs").read_text(encoding="utf-8")
    assert "const STEP_LIMIT: usize = 4096" in source
    assert "TOTAL_INSTRUCTION_LIMIT: u64 = 1_000_000" in source
    assert "UnsupportedFrameworkApi" in source
    assert "return Err(DalvikVmError::UnsupportedFrameworkApi)" in source



def test_manifest_identity_is_never_synthesized():
    source = (ROOT / "kernel/src/compat/android/apk.rs").read_text(encoding="utf-8")
    assert 'String::from("com.unknown.android_app")' not in source
    assert "if package_name.is_empty()" in source
    assert "return Err(ApkError::MalformedXmlChunk)" in source
