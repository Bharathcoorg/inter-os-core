# Gemma 4 E2B Runtime Compatibility & Feasibility Analysis

**Platform:** InterOS Core (Microkernel, Capability Security, `#![no_std]` Userspace)  
**Evaluation Scope:** Google DeepMind Official Checkpoints  
**Date:** 2026-11-19  

---

## 1. Google Mobile QAT Checkpoint (`model.safetensors`)

### Technical Profile
- **Container:** Safetensors (2,780 tensors, 2.46 GB).
- **Quantization:** Google custom `wNa8o8` (mixed 2-bit, 4-bit, and 8-bit integer weights with INT8 dynamic or static activations).
- **Scale Layout:** Each quantized layer features independent per-channel float32 tensors:
  * `<layer>.weight_scale` (vector matching output channel count)
  * `<layer>.input_activation_scale` (scalar float32)
  * `<layer>.output_activation_scale` (scalar float32)

### Operator & Decode Requirements
1. **Integer Dequantization:** Requires bit-unpacking for 2-bit and 4-bit weights packed into `u8` arrays.
2. **Quantized GEMM / GEMV:** INT8 × INT4 -> INT32 accumulation with runtime float32 scale requantization:
   $$\text{Output}_i = \text{Scale}_{\text{input}} \times \text{Scale}_{\text{weight}, i} \times \sum_j (X_j \cdot W_{i,j})$$
3. **Activation Format:** Symmetric signed 8-bit integer (`i8`) activations with clamping to `[-128, 127]`.
4. **Attention Mechanism:** Multi-Query Attention with 20 shared-KV layers and 512-token sliding window RoPE.

### InterOS Userspace Viability
- **Feasibility:** High for future dedicated native Rust engine, but **NOT compatible with existing GGUF parser**.
- **Assessment:** Safetensors parsing is straightforward in Rust (`serde_json` or custom flat parser). However, executing `wNa8o8` in `#![no_std]` requires implementing dedicated SIMD-accelerated INT8/INT4 matrix-vector kernels (AVX2/AVX-512 for x86_64, NEON/DotProd/I8MM for aarch64). This represents a distinct inference runtime path from GGML/GGUF.

---

## 2. LiteRT-LM Deployment Package (`gemma-4-E2B-it.litertlm`)

### Technical Profile
- **Container:** `LITERTLM` version 1 binary archive (2.59 GB).
- **Internal Structure:** FlatBuffer directory embedding 10 pre-compiled TensorFlow Lite subgraphs:
  * `tf_lite_embedder`
  * `tf_lite_per_layer_embedder`
  * `tf_lite_prefill_decode`
  * `tf_lite_mtp_drafter`
  * Audio & Vision encoders/adapters

### Runtime Dependencies & Ecosystem Requirements
- **Host Runtime:** Google LiteRT-LM C++ runtime (`liblitert_lm.so` / `litert_lm.dll`).
- **Dependencies:**
  * C++20 standard library (`libc++` / `libstdc++`)
  * Dynamic linker / loader (`dlopen`, `dlsym`)
  * POSIX threading (`pthread`)
  * System memory virtual allocator (`mmap`, `mprotect`)
  * XNNPACK CPU acceleration library
  * OpenCL / Vulkan / Android NNAPI / Qualcomm QNN delegates

### InterOS Userspace Viability
- **Current Userspace Barrier:** InterOS Ring-3 processes are strictly `#![no_std]` and `#![no_main]`, executing on raw microkernel syscalls (`sys_alloc`, `sys_ipc_send`, `sys_ipc_recv`) with zero POSIX or C runtime emulation.
- **Porting Requirements:** To run LiteRT-LM directly inside InterOS userspace, the platform would require:
  1. A C/C++ runtime compatibility layer (`musl` or `picolibc` port).
  2. Dynamic ELF linking or massive static compilation of the LiteRT-LM engine.
  3. System threading and futex wrappers wired to InterOS kernel futex syscalls.
- **Verdict:** Highly valuable as an **independent host reference engine**, but cannot run directly in the current bare-metal InterOS guest userspace without substantial C runtime infrastructure.

---

## 3. Official Google QAT Q4_0 GGUF Checkpoint (`gemma-4-E2B_q4_0-it.gguf`)

### Technical Profile
- **Format:** GGUF v3 (3.35 GB, 541 tensors, 49 KV metadata entries).
- **Architecture:** `gemma4`, 35 layers, 1536 hidden dimension, 8 heads, 1 KV head, 262,144 vocabulary.
- **Tensor Dtypes:**
  * `dtype 0` (F32): 263 tensors (Norms, Scales) — **SUPPORTED** (in InterOS decode set)
  * `dtype 1` (F16): 1 tensor — **SUPPORTED** (in InterOS decode set)
  * `dtype 2` (Q4_0): 275 tensors (Transformer Blocks) — **SUPPORTED** (in InterOS decode set)
  * `dtype 14` (Q6_K): 2 tensors (`token_embd.weight`, `per_layer_token_embd.weight`) — **CURRENTLY UNMAPPED**

### Exact Compatibility Gaps with InterOS Core `aid` Service

1. **Arithmetic Decode Set Gap (`dtype 14` / Q6_K):**
   * InterOS Core's decode set is pinned to `[0, 1, 2, 8, 12]` (F32, F16, Q4_0, Q8_0, Q4_K).
   * Google's official GGUF quantized the transformer block matrices in `dtype 2` (Q4_0), which our engine reads cleanly.
   * However, Google quantized the token embedding table (`token_embd.weight`) and the per-layer embedding table (`per_layer_token_embd.weight`) in `dtype 14` (Q6_K: 256-element superblocks, 210 bytes/block, 6.5625 bits/weight).
   * Because `dtype 14` is not in `[0, 1, 2, 8, 12]`, `quantised_row_bytes(14, 1536)` returns `None`, refusing model admission.

2. **Tied Embedding Output Head:**
   * In standard Gemma 4, `output.weight` is absent; the model shares weights with `token_embd.weight`.
   * InterOS Core's `run_generative_model` currently expects `reader.has_output_head()` which looks specifically for `TENSOR_OUTPUT_HEAD = "output.weight"`.
   * For Gemma models with tied embeddings, the projection must resolve to `token_embd.weight`.

3. **Per-Layer Input Embedding (PLE):**
   * Gemma 4 E2B incorporates a per-layer input embedding table `per_layer_token_embd.weight` of dimension `[8960, 262144]`.
   * Sized at 1,321,205,760 bytes (1.32 GB), loading this tensor into memory violates the single-mapping ceiling (134 MB). It must be accessed via streamed, position-addressed 64 KiB reads (`load_per_layer_token_embd_row`).

4. **Shared KV Attention Structure:**
   * 20 of the 35 layers (layers 15..34) share key and value representations from earlier layers, omitting `attn_k.weight` and `attn_v.weight`.
   * The block loader must map shared KV indices rather than failing with `LoadError::TensorAbsent`.

---

## 4. Execution Feasibility Roadmap

| Milestone | Action Required | Feasibility & Safety |
|---|---|---|
| **Phase 1: Acquisition & Verification** | Download official models, compute SHA-256, document provenance. | **COMPLETE (Done)** |
| **Phase 2: Host Reference Testing** | Validate numerical token generation on host via official Hugging Face / LiteRT runtime. | **HIGH (Ready to run)** |
| **Phase 3: Q6_K Decoder Implementation** | Implement correct `dequantize_q6_k` and `gemv_q6_k` with unit tests for `dtype 14`. | **Requires deliberate engineering & numerical verification** |
| **Phase 4: Tied Embeddings & Shared KV** | Support tied output projection and 20 shared-KV layers in `weights.rs`. | **Straightforward architectural extension** |
| **Phase 5: Disposable Guest Provisioning** | Provision verified model onto a disposable test disk for QEMU guest execution. | **Follows standard InterFS toolchain** |
