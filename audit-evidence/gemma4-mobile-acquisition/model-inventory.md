# Official Gemma 4 E2B Physical Model Inventory & Provenance

**Timestamp:** 2026-11-19T19:15:00+05:30  
**Repository:** `Bharathcoorg/inter-os-core`  
**Platform Truth Contract Status:** 100% Verified from Real File Bytes  

---

## 1. Overview of Acquired Official Artifacts

Three official Gemma 4 E2B model repositories were acquired directly from Google DeepMind and LiteRT official Hugging Face distributions. All file bytes were streamed, verified against Git LFS pointer text, and validated via full SHA-256 hashes.

```
models/downloads/
├── gemma4-mobile-qat/         # Google mobile-optimised QAT Transformers weights
│   ├── config.json
│   ├── generation_config.json
│   ├── chat_template.jinja
│   ├── tokenizer.json
│   ├── tokenizer_config.json
│   ├── processor_config.json
│   ├── preprocessor_config.json
│   ├── model.safetensors      # 2,458,111,846 bytes (2.46 GB)
│   └── README.md
├── gemma4-litert/             # LiteRT-LM mobile deployment model
│   ├── gemma-4-E2B-it.litertlm# 2,588,147,712 bytes (2.59 GB)
│   ├── chat_template.jinja
│   └── README.md
└── gemma4-official-gguf/      # Official Google QAT Q4_0 GGUF checkpoint
    ├── gemma-4-E2B_q4_0-it.gguf # 3,349,516,256 bytes (3.35 GB)
    └── README.md
```

---

## 2. Artifact A: Google Mobile-Optimised QAT Transformers Checkpoint

- **Upstream Repository:** `google/gemma-4-E2B-it-qat-mobile-transformers`
- **Publisher:** Google DeepMind
- **Repository Commit:** `dd693ff40353f057ca5f07e945ad867f4afbf2ec`
- **License:** Gemma Terms of Use / Apache 2.0
- **Primary Binary:** `model.safetensors`
  * **Size:** 2,458,111,846 bytes (2,344.24 MiB)
  * **Full SHA-256:** `efab429012b97ab986c4d4838a46ff3ad95d618b42ce514771ca40fadc76a9a4`
  * **LFS Check:** VERIFIED real payload (header length 2,780 keys, not LFS pointer)
- **Tokenizer:** `tokenizer.json`
  * **Size:** 32,169,626 bytes (30.68 MiB)
  * **Full SHA-256:** `cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f`
- **Architecture & Specifications:**
  * Model Family: Gemma 4 Multimodal / Text
  * Architecture: `gemma4`
  * Parameter Count: ~4.6 Billion effective parameters (2.3 Billion non-embedding parameters)
  * Layers: 35 transformer blocks (sliding attention window: 512, full attention every 5th layer)
  * Hidden Size: 1,536
  * Intermediate (FFN) Size: 6,144
  * Attention Heads: 8 (Head Dimension: 256)
  * KV Heads: 1 (Multi-Query / Grouped Query Attention)
  * Shared KV Layers: 20
  * Per-Layer Input Dimension: 256
  * Vocabulary: 262,144 tokens
- **Quantization Scheme:**
  * Method: Google Gemma custom QAT `wNa8o8` (Weight N-bit, Activation 8-bit, Output 8-bit)
  * LM Head: 2-bit
  * Embeddings: 2-bit / 4-bit
  * MLP layers: 4-bit (early layers 0..14) and 2-bit (late layers 15..34)
  * Self-Attention: 4-bit
  * Scales: Explicit per-channel float32 weight scales, input activation scales, output activation scales
- **Tensor Inventory:** 2,780 tensors in `model.safetensors`:
  * F32: 1,869 tensors (scales, layer norms, RMS norms)
  * BF16: 361 tensors (residual scales, unquantized heads)
  * I8: 182 tensors
  * U8: 368 tensors (packed quantized weights)

---

## 3. Artifact B: LiteRT-LM Deployment Package

- **Upstream Repository:** `litert-community/gemma-4-E2B-it-litert-lm`
- **Publisher:** LiteRT Community / Google
- **Repository Commit:** `2d74ac41a33d7da82f39f7d5720a04c68514a103`
- **License:** Apache 2.0 / Gemma Terms of Use
- **Primary Binary:** `gemma-4-E2B-it.litertlm`
  * **Size:** 2,588,147,712 bytes (2,468.25 MiB)
  * **Full SHA-256:** `181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c`
  * **LFS Check:** VERIFIED real payload (starts with magic `LITERTLM\x01\x00\x00\x00`)
- **Format:** LiteRT-LM multi-submodel container (FlatBuffers + TFLite modules)
- **Sub-Models Packed:**
  1. `tf_lite_embedder`: Token embedding lookup graph
  2. `tf_lite_per_layer_embedder`: Gemma 4 per-layer input embedding graph
  3. `tf_lite_prefill_decode`: Primary autoregressive prefill & decode engine
  4. `tf_lite_mtp_drafter`: Multi-Token Prediction speculative decoding draft engine
  5. `tf_lite_audio_encoder_hw` & `tf_lite_audio_adapter`: Audio feature encoder
  6. `tf_lite_vision_encoder` & `tf_lite_vision_adapter`: Vision feature encoder
- **Execution Target:** Google LiteRT runtime with XNNPACK CPU backend and OpenCL/Vulkan/NPU delegates.

---

## 4. Artifact C: Official Google QAT Q4_0 GGUF Checkpoint

- **Upstream Repository:** `google/gemma-4-E2B-it-qat-q4_0-gguf`
- **Publisher:** Google DeepMind
- **Repository Commit:** `675cff42a74c774d6cb76f76d8eacb49b48c9b93`
- **License:** Apache 2.0 / Gemma Terms of Use
- **Primary Binary:** `gemma-4-E2B_q4_0-it.gguf`
  * **Size:** 3,349,516,256 bytes (3,194.35 MiB)
  * **Full SHA-256:** `fa401b55b07ee70a54c6dae3903c783a6e65064312529ea57175cb5f8dec6634`
  * **LFS Check:** VERIFIED real payload (GGUF magic `0x46554747`, version 3)
- **Metadata Geometry:**
  * Architecture (`general.architecture`): `gemma4`
  * Size Label (`general.size_label`): `4.6B`
  * Transformer Layers (`gemma4.block_count`): 35
  * Context Length (`gemma4.context_length`): 131,072
  * Hidden Dimension (`gemma4.embedding_length`): 1,536
  * Attention Heads (`gemma4.attention.head_count`): 8
  * KV Attention Heads (`gemma4.attention.head_count_kv`): 1
  * Shared KV Layers (`gemma4.attention.shared_kv_layers`): 20
  * Per-Layer Input Dimension (`gemma4.embedding_length_per_layer_input`): 256
  * Logit Soft-Capping: 30.0
  * RoPE Theta Base: 1,000,000.0 (Sliding Window RoPE: 10,000.0)
- **Tensor Count:** 541 tensors
- **Tensortype Census:**
  * **dtype 0 (F32):** 263 tensors (RMS norms, post-attention norms, layer output scales)
  * **dtype 1 (F16):** 1 tensor
  * **dtype 2 (Q4_0):** 275 tensors (all attention and FFN projection matrices across all 35 layers)
  * **dtype 14 (Q6_K):** 2 tensors (`per_layer_token_embd.weight` and `token_embd.weight`)
- **Output Head:**
  * `output.weight` is absent as a discrete tensor; output logits use tied embeddings with `token_embd.weight` (Q6_K).
