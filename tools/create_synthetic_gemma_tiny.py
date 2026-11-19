#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
Deterministic Generator for InterOS Synthetic Gemma Fixture (gemma-tiny-f32.gguf)
================================================================================
Generates the exact 521,536-byte reference fixture required by:
- tests/test_model_acceptance_e2b.py (Point 16)
- tests/test_model_acceptance_eg2.py (Point 15)
- userspace/system-service/src/weights.rs (test_the_physical_gemma_tiny_generative_model_loads_and_generates_tokens)
- tests/test_qemu_inference_acceptance.py (/data/models/gemma4-mobile-lite.gguf)

Specifications:
- Format: GGUF v3
- Total Bytes: 521,536
- Architecture: gemma2
- Name: InterOS synthetic Gemma fixture
- Size Label: tiny-f32
- Parameters: 128K params, F32, 2 layers, hidden_dim 64, vocab 290
- Tensor Count: 31 tensors (26 block tensors + 5 outside tensors)
"""

import math
import os
import struct
import sys

TARGET_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "target",
    "verify",
    "models",
    "gemma-tiny-f32.gguf"
)
TOTAL_BYTES = 521_536
ALIGNMENT = 32

def write_str(s: str | bytes) -> bytes:
    b = s.encode("utf-8") if isinstance(s, str) else s
    return struct.pack("<Q", len(b)) + b

def write_kv_str(k: str, v: str) -> bytes:
    return write_str(k) + struct.pack("<I", 8) + write_str(v)

def write_kv_u32(k: str, v: int) -> bytes:
    return write_str(k) + struct.pack("<I", 4) + struct.pack("<I", v)

def write_kv_f32(k: str, v: float) -> bytes:
    return write_str(k) + struct.pack("<I", 6) + struct.pack("<f", v)

def write_kv_array_str(k: str, strings: list[str]) -> bytes:
    res = bytearray()
    res += write_str(k)
    res += struct.pack("<I", 9)  # val_type = ARRAY
    res += struct.pack("<I", 8)  # elem_type = STRING
    res += struct.pack("<Q", len(strings))
    for s in strings:
        res += write_str(s)
    return bytes(res)

def write_kv_array_f32(k: str, floats: list[float]) -> bytes:
    res = bytearray()
    res += write_str(k)
    res += struct.pack("<I", 9)  # val_type = ARRAY
    res += struct.pack("<I", 6)  # elem_type = FLOAT32
    res += struct.pack("<Q", len(floats))
    for f in floats:
        res += struct.pack("<f", f)
    return bytes(res)

def generate_tiny_fixture(output_path: str = TARGET_PATH) -> str:
    os.makedirs(os.path.dirname(output_path), exist_ok=True)

    # 1. Build Vocabulary: 290 tokens
    vocab = ["<pad>", "<eos>", "<bos>", "<unk>"]
    for i in range(256):
        vocab.append(f"<0x{i:02X}>")
    for i in range(290 - len(vocab)):
        vocab.append(f"w{i}")
    assert len(vocab) == 290

    scores = [-10.0] * 290
    scores[1] = 0.0   # <eos>
    scores[2] = 0.0   # <bos>

    # 2. Build Tensors Specification: exactly 31 tensors
    tensors_spec = []
    # Outside tensors (5)
    tensors_spec.append(("token_embd.weight", [64, 290], 64 * 290 * 4))
    tensors_spec.append(("output_norm.weight", [64], 64 * 4))
    tensors_spec.append(("output.weight", [64, 290], 64 * 290 * 4))
    tensors_spec.append(("dense_2.weight", [64, 64], 64 * 64 * 4))
    tensors_spec.append(("dense_3.weight", [64, 64], 64 * 64 * 4))

    # Per-block tensors (13 per block * 2 blocks = 26)
    for b in range(2):
        tensors_spec.append((f"blk.{b}.attn_q.weight", [64, 64], 64 * 64 * 4))
        tensors_spec.append((f"blk.{b}.attn_k.weight", [64, 16], 64 * 16 * 4))
        tensors_spec.append((f"blk.{b}.attn_v.weight", [64, 16], 64 * 16 * 4))
        tensors_spec.append((f"blk.{b}.attn_output.weight", [64, 64], 64 * 64 * 4))
        tensors_spec.append((f"blk.{b}.ffn_gate.weight", [64, 128], 64 * 128 * 4))
        tensors_spec.append((f"blk.{b}.ffn_up.weight", [64, 128], 64 * 128 * 4))
        tensors_spec.append((f"blk.{b}.ffn_down.weight", [128, 64], 128 * 64 * 4))
        tensors_spec.append((f"blk.{b}.attn_norm.weight", [64], 64 * 4))
        tensors_spec.append((f"blk.{b}.attn_q_norm.weight", [16], 16 * 4))
        tensors_spec.append((f"blk.{b}.attn_k_norm.weight", [16], 16 * 4))
        tensors_spec.append((f"blk.{b}.post_attention_norm.weight", [64], 64 * 4))
        tensors_spec.append((f"blk.{b}.ffn_norm.weight", [64], 64 * 4))
        tensors_spec.append((f"blk.{b}.post_ffw_norm.weight", [64], 64 * 4))

    assert len(tensors_spec) == 31
    tensors_payload_bytes = sum(b for _, _, b in tensors_spec)
    assert tensors_payload_bytes == 462_336, f"Payload: {tensors_payload_bytes}"

    target_data_start = TOTAL_BYTES - tensors_payload_bytes
    assert target_data_start == 59_200
    assert target_data_start % ALIGNMENT == 0

    # 3. Build Metadata KV entries
    kv_list = [
        write_kv_str("general.architecture", "gemma2"),
        write_kv_str("general.name", "InterOS synthetic Gemma fixture"),
        write_kv_str("general.size_label", "tiny-f32"),
        write_kv_u32("general.alignment", ALIGNMENT),
        write_kv_u32("general.file_type", 0),  # 0 = ALL_F32
        write_kv_u32("gemma2.block_count", 2),
        write_kv_u32("gemma2.embedding_length", 64),
        write_kv_u32("gemma2.feed_forward_length", 128),
        write_kv_u32("gemma2.attention.head_count", 4),
        write_kv_u32("gemma2.attention.head_count_kv", 1),
        write_kv_u32("gemma2.attention.key_length", 16),
        write_kv_u32("gemma2.context_length", 64),
        write_kv_f32("gemma2.attention.layer_norm_rms_epsilon", 1e-6),
        write_kv_f32("gemma2.rope.freq_base", 10000.0),
        write_kv_u32("tokenizer.ggml.bos_token_id", 2),
        write_kv_u32("tokenizer.ggml.eos_token_id", 1),
        write_kv_u32("tokenizer.ggml.padding_token_id", 0),
        write_kv_u32("tokenizer.ggml.unknown_token_id", 3),
        write_kv_str("tokenizer.ggml.model", "gemma2"),
        write_kv_array_str("tokenizer.ggml.tokens", vocab),
        write_kv_array_f32("tokenizer.ggml.scores", scores),
    ]

    # Calculate descriptor table size
    # For each tensor:
    # write_str(name) + n_dims (u32: 4) + dims (n_dims * u64: 8 * n) + dtype (u32: 4) + offset (u64: 8)
    descriptor_bytes = 0
    for name, dims, _ in tensors_spec:
        descriptor_bytes += 8 + len(name.encode("utf-8")) + 4 + (len(dims) * 8) + 4 + 8

    # Base header size: 4 (GGUF) + 4 (ver) + 8 (tensor_count) + 8 (kv_count) = 24 bytes
    header_fixed_bytes = 24
    base_kv_bytes = sum(len(kv) for kv in kv_list)
    current_table_end = header_fixed_bytes + base_kv_bytes + descriptor_bytes

    # We add a padding string metadata key "general.padding" to make (table_end + 31) // 32 * 32 == target_data_start
    # write_kv_str("general.padding", pad_str) costs:
    # 8 + 15 (key) + 4 (type=8) + 8 + len(pad_str) = 35 + len(pad_str)
    pad_key = "general.padding"
    overhead = 8 + len(pad_key) + 4 + 8  # 35 bytes
    needed_total_kv_and_pad = target_data_start - descriptor_bytes - header_fixed_bytes
    pad_len = needed_total_kv_and_pad - base_kv_bytes - overhead
    assert pad_len >= 0, f"Cannot pad: table already too large ({current_table_end} > {target_data_start})"

    pad_str = "x" * pad_len
    kv_list.append(write_kv_str(pad_key, pad_str))

    # Re-calculate table_end
    kv_data = b"".join(kv_list)
    kv_count = len(kv_list)
    header = struct.pack("<4sIQQ", b"GGUF", 3, len(tensors_spec), kv_count)

    # Build descriptor table
    descriptors = bytearray()
    cur_offset = 0
    for name, dims, byte_len in tensors_spec:
        descriptors += write_str(name)
        descriptors += struct.pack("<I", len(dims))
        for d in dims:
            descriptors += struct.pack("<Q", d)
        descriptors += struct.pack("<I", 0)  # dtype = 0 (F32)
        descriptors += struct.pack("<Q", cur_offset)
        cur_offset += byte_len

    table_end = len(header) + len(kv_data) + len(descriptors)
    assert table_end == target_data_start, f"table_end {table_end} != {target_data_start}"

    # 4. Generate Tensor Payloads (deterministic non-zero values)
    data_payload = bytearray()
    for name, dims, byte_len in tensors_spec:
        elem_count = 1
        for d in dims:
            elem_count *= d
        t_data = bytearray(byte_len)

        # Norms set to 1.0; weights set to small stable values
        if "norm" in name:
            floats = [1.0] * elem_count
        elif "dense" in name or "ffn" in name or "attn" in name or "embd" in name or "output" in name:
            floats = [
                0.02 * math.sin(i * 0.05 + 0.1) for i in range(elem_count)
            ]
        else:
            floats = [0.01] * elem_count

        struct.pack_into(f"<{elem_count}f", t_data, 0, *floats)
        data_payload += t_data

    assert len(data_payload) == tensors_payload_bytes

    total_file = header + kv_data + bytes(descriptors) + bytes(data_payload)
    assert len(total_file) == TOTAL_BYTES, f"File size {len(total_file)} != {TOTAL_BYTES}"

    with open(output_path, "wb") as f:
        f.write(total_file)

    print(f"[OK] Successfully generated {output_path} ({len(total_file)} bytes).")
    return output_path

if __name__ == "__main__":
    generate_tiny_fixture()
