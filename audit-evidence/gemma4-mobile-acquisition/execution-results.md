# Gemma 4 E2B Acquisition & Physical Evidence Report

**Project:** InterOS Core  
**Branch:** `master`  
**Milestone:** Official Gemma 4 E2B Acquisition & Provenance Verification  
**Completion Date:** 2026-11-19  

---

## 1. Executive Summary

In accordance with the engineering directives, genuine, complete, instruction-tuned Gemma 4 E2B model artifacts have been acquired directly from Google DeepMind's official distributions on Hugging Face. The previous `mtp-gemma-4-E2B-it.gguf` file (56.5 MB) was proven to be a Multi-Token Prediction speculative drafting adapter; it has been superseded by genuine full-model checkpoints.

All artifacts were downloaded using verified HTTP streaming via `huggingface_hub`, confirmed not to be Git LFS pointer stubs, and verified with full SHA-256 cryptographic digests.

---

## 2. Acquired Artifacts & Provenance Master Table

| Canonical Identifier | Local Relative Path | Byte Size | Full SHA-256 Checksum | Format / Source |
|---|---|---|---|---|
| **Google Mobile QAT Checkpoint** | `models/downloads/gemma4-mobile-qat/model.safetensors` | 2,458,111,846 B | `efab429012b97ab986c4d4838a46ff3ad95d618b42ce514771ca40fadc76a9a4` | Safetensors / `google/gemma-4-E2B-it-qat-mobile-transformers` (`dd693ff`) |
| **Gemma 4 Mobile Tokenizer** | `models/downloads/gemma4-mobile-qat/tokenizer.json` | 32,169,626 B | `cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f` | JSON BPE / `google/gemma-4-E2B-it-qat-mobile-transformers` (`dd693ff`) |
| **LiteRT-LM Deployment Package** | `models/downloads/gemma4-litert/gemma-4-E2B-it.litertlm` | 2,588,147,712 B | `181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c` | LITERTLM v1 / `litert-community/gemma-4-E2B-it-litert-lm` (`2d74ac4`) |
| **Google Official QAT GGUF** | `models/downloads/gemma4-official-gguf/gemma-4-E2B_q4_0-it.gguf` | 3,349,516,256 B | `fa401b55b07ee70a54c6dae3903c783a6e65064312529ea57175cb5f8dec6634` | GGUF v3 / `google/gemma-4-E2B-it-qat-q4_0-gguf` (`675cff4`) |

---

## 3. Storage Safety & Git Quarantine Verification

In strict compliance with repository safety directives:
1. **Zero Git Bloat:** No binary model files (`.safetensors`, `.litertlm`, `.gguf`) have been staged or committed to Git. All binaries reside exclusively in the host-side directory `models/downloads/`.
2. **Master Disk Protection:** The persistent master disk (`target/inter-os-data-master.img`) was never mounted or provisioned during this acquisition process.
3. **Host Storage Accounting:**
   * **Free Space Before Acquisition:** 12.26 GB
   * **Total Acquired Payload:** 8.43 GB
   * **Free Space After Acquisition:** 4.00 GB
4. **Git LFS Integrity:** Every downloaded file was inspected and verified to contain binary neural network weights, not ASCII LFS pointer text (`version https://git-lfs...`).

---

## 4. Platform Truth Findings & Gap Identification

1. **Standalone Model Reality:**
   * The newly acquired `gemma-4-E2B_q4_0-it.gguf` (3.35 GB) is a genuine 35-block, 541-tensor Gemma 4 E2B foundation model.
   * Unlike the 56.5 MB drafting adapter, it contains the complete transformer backbone with 275 Q4_0 projection matrices across all 35 layers.
2. **Decode Set Discipline:**
   * In `gemma-4-E2B_q4_0-it.gguf`, Google encoded the two massive embedding tables (`token_embd.weight` and `per_layer_token_embd.weight`) using `dtype 14` (Q6_K).
   * InterOS Core's arithmetic decode set is pinned to `[0, 1, 2, 8, 12]`.
   * Per the Platform Truth Contract, we report this gap truthfully: InterOS Core cannot execute this official GGUF model in the guest until a verified `Q6_K` dequantizer and tied-embedding projection are implemented with full unit-test coverage.
3. **LiteRT-LM Feasibility:**
   * LiteRT-LM requires a full C++20 runtime, dynamic linking, and POSIX threads, which are absent in `#![no_std]` InterOS userspace. It serves as an authoritative host-side execution baseline.

---

## 5. Host Reference Numerical Inference Results

Using the official tool `tools/test_host_reference_gemma4.py`, genuine numerical execution was performed against `models/downloads/gemma4-mobile-qat` on the host CPU with deterministic decoding (`do_sample=False`).

### Execution Summary
- **Execution Engine:** Host-backed PyTorch 2.13.0+cpu & Transformers 5.18.0 (`Gemma4ForConditionalGeneration`).
- **Prompt:** `"Explain capability security in one sentence."`
- **Chat Template Tokens (16 tokens):**  
  `[2, 105, 2364, 107, 155122, 20478, 5052, 528, 886, 13315, 236761, 106, 107, 105, 4368, 107]`  
  (`bos`, `<start_of_turn>user\nExplain capability security in one sentence.<end_of_turn>\n<start_of_turn>model\n`)
- **Generated Output Tokens (28 tokens):**  
  `[224287, 5052, 563, 506, 5428, 529, 17096, 600, 1186, 19278, 5089, 653, 6585, 740, 2802, 532, 1161, 3530, 15858, 236764, 17534, 24284, 13719, 1262, 532, 5405, 236761, 106]`
- **Decoded Text:**  
  `"Capability security is the practice of ensuring that only authorized users or processes can access and use specific capabilities, thereby protecting sensitive data and resources."`
- **End-of-Sequence:** Properly emitted token `106` (`<end_of_turn>`).
- **Latency & Throughput:**
  * Model weight loading: 5.87 seconds
  * Generation runtime: 247.56 seconds (~0.11 tokens/sec on multi-core CPU)
- **Labeling (Platform Truth Contract):** Explicitly documented as **host-backed reference inference**, establishing the mathematical and conversational authenticity of Google's official model weights before guest integration.

---

## 6. Next Steps

1. Maintain binary model quarantine in `models/downloads/` (zero Git bloat).
2. For on-device guest inference:
   * Implement verified `Q6_K` dequantizer and tied embeddings in `userspace/system-service/src/inference.rs` and `weights.rs` with dedicated mathematical unit tests.
   * Expand the arithmetic decode set to include `14` (Q6_K) once verified against contract tests.
   * Provision the official GGUF onto a disposable QEMU test disk and execute guest inference.
