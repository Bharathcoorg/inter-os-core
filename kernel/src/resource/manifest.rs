// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Resource Manifest for adaptive scheduling and admission control.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[repr(C)]
pub struct ResourceManifest {
    pub peak_scratch_bytes: u64,
    pub kv_cache_bytes: u64,
    pub peak_activations_bytes: u64,
    pub tensor_alignment: u32,
    pub quantization_format: u32,
}
