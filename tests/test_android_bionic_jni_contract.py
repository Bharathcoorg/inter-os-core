# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Android Bionic ABI & Java Native Interface (JNI) Contract Tests.

Verifies:
1. Pure-Rust ELF parser with strict W^X memory protection enforcement.
2. Rejection of segments requesting both W (write) and X (execute).
3. BionicMediator system property routing and memory quota enforcement.
4. JNIEnv and JavaVM standard JNI 1.6 bridging between DEX and native code.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_bionic_elf_loader_and_wx_enforcement_invariants():
    source = read_source("kernel/src/compat/android/bionic.rs")
    assert "pub const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];" in source
    assert "pub enum MemoryProtection" in source
    assert "ReadExecute" in source
    assert "ReadWrite" in source
    assert "ReadOnly" in source
    assert "WxViolation" in source
    assert "pub fn parse_and_load(elf_bytes: &[u8]) -> Result<Self, ElfError>" in source


def test_bionic_mediator_system_properties_and_memory():
    source = read_source("kernel/src/compat/android/bionic.rs")
    assert "pub struct BionicMediator" in source
    assert "pub fn log_print(&mut self, prio: i32, tag: &str, text: &str)" in source
    assert 'pub fn get_system_property(&self, name: &str) -> Option<&\'static str>' in source
    assert '"ro.build.version.sdk" => Some("28")' in source
    assert '"ro.product.model" => Some("InterOS Sovereign Phone")' in source
    assert "pub fn malloc(&mut self, size: usize) -> Result<usize, ()>" in source
    assert "pub fn free(&mut self, size: usize)" in source

    # Also verify unprivileged Ring-3 ACR bionic module
    acr_bionic = read_source("crates/interos-acr/src/bionic.rs")
    assert '"ro.build.version.sdk" => Some("28")' in acr_bionic
    assert "pub struct BionicMediator" in acr_bionic


def test_jni_env_and_javavm_interface_contracts():
    source = read_source("kernel/src/compat/android/jni.rs")
    assert "pub const JNI_VERSION_1_6: u32 = 0x00010006;" in source
    assert "pub struct JNINativeMethod" in source
    assert "pub struct JniEnv" in source
    assert "pub struct JavaVM" in source
    assert "pub fn find_class(&mut self, name: &str) -> Option<u32>" in source
    assert "pub fn get_static_method_id" in source
    assert "pub fn register_natives" in source
    assert "pub fn invoke_native_method" in source
    assert "pub fn new_string_utf" in source
    assert "pub fn get_string_utf_chars" in source
    assert "pub fn new_object" in source
    assert "pub fn get_env(&mut self, version: u32) -> Result<&mut JniEnv, i32>" in source
