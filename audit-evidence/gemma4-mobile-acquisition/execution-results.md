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

## 5. Next Steps

1. Commit only the audit manifests, acquisition tooling, and documentation under `audit-evidence/gemma4-mobile-acquisition/` and `tools/download_official_gemma4.py`.
2. Keep all model binary files quarantined in `models/downloads/`.
3. Plan the Phase 2 implementation of the Q6_K dequantizer for InterOS Core's system service if on-device execution of the official GGUF artifact is desired.
