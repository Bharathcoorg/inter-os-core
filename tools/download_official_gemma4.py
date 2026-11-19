#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Fetch or verify the LiteRT-LM Gemma 4 E2B package and optional host references."""

import argparse
import os
import sys
import time
import hashlib

DOWNLOADS = [
    {
        "repo_id": "google/gemma-4-E2B-it-qat-mobile-transformers",
        "role": "reference",
        "revision": "dd693ff40353f057ca5f07e945ad867f4afbf2ec",
        "weights": "model.safetensors",
        "sha256": "efab429012b97ab986c4d4838a46ff3ad95d618b42ce514771ca40fadc76a9a4",
        "local_dir": "models/downloads/gemma4-mobile-qat",
        "files": [
            "config.json",
            "generation_config.json",
            "chat_template.jinja",
            "tokenizer_config.json",
            "tokenizer.json",
            "processor_config.json",
            "preprocessor_config.json",
            "model.safetensors",
            "README.md",
        ]
    },
    {
        "repo_id": "litert-community/gemma-4-E2B-it-litert-lm",
        "revision": "2d74ac41a33d7da82f39f7d5720a04c68514a103",
        "weights": "gemma-4-E2B-it.litertlm",
        "sha256": "181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c",
        "role": "generation",
        "local_dir": "models/downloads/gemma4-litert",
        "files": [
            "README.md",
            "chat_template.jinja",
            "gemma-4-E2B-it.litertlm",
        ]
    },
    {
        "repo_id": "google/gemma-4-E2B-it-qat-q4_0-gguf",
        "role": "reference",
        "revision": "675cff42a74c774d6cb76f76d8eacb49b48c9b93",
        "weights": "gemma-4-E2B_q4_0-it.gguf",
        "sha256": "fa401b55b07ee70a54c6dae3903c783a6e65064312529ea57175cb5f8dec6634",
        "local_dir": "models/downloads/gemma4-official-gguf",
        "files": [
            "README.md",
            "gemma-4-E2B_q4_0-it.gguf",
        ]
    },
    {
        "repo_id": "litert-community/embeddinggemma-2-text-270m-litert-lm",
        "revision": "9be6e8b90982095dc05c2bd162e4b954ee4dbac7",
        "weights": "embeddinggemma-2-text-270m.litertlm",
        "sha256": "2d079ee2f6f066b1f368e8d7c819f55214eaef1d0513b312321901f30ab286fb",
        "local_dir": "models/downloads/embeddinggemma2-text",
        "files": ["embeddinggemma-2-text-270m.litertlm"],
        "role": "embedding",
    }
]

def log(msg):
    print(msg, flush=True)

def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()

def verify_not_lfs_pointer(path):
    size = os.path.getsize(path)
    if size < 512:
        with open(path, "rb") as f:
            head = f.read(100)
            if b"version https://git-lfs.github.com/spec/v1" in head:
                return False
    return True


def write_model_manifest(item, artifact_path, size, digest):
    """Writes data consumed by the guest model registry, never a model binary."""
    if item["role"] not in {"generation", "embedding"}:
        return None
    if item["role"] == "generation":
        model_id = "org.interos.gemma4-e2b-it"
        name = "Gemma 4 E2B IT"
        architecture = "gemma4-e2b"
        capabilities = "text-generation,streaming-generation,vision-input"
    else:
        model_id = "org.interos.embeddinggemma2-text-270m"
        name = "EmbeddingGemma 2 Text 270M"
        architecture = "embeddinggemma2"
        capabilities = "text-embedding"
    # Zero resource fields mean that the installed backend must derive the
    # estimate by probing the container. They are not claims that a run needs
    # no memory, and aid will not admit a zero estimate as executable state.
    content = "\n".join([
        f"id={model_id}",
        f"name={name}",
        f"source={item['repo_id']}",
        f"revision={item['revision']}",
        f"sha256={digest}",
        f"artifact_bytes={size}",
        "format=litert-lm",
        f"architecture={architecture}",
        "quantization=backend-defined",
        "tokenizer=embedded",
        "hardware=cpu",
        f"capabilities={capabilities}",
        "memory.model_weights=0",
        "memory.kv_cache=0",
        "memory.activations=0",
        "memory.scratch=0",
        "memory.tokenizer=0",
        "memory.embedding_indexes=0",
        "memory.audio_buffers=0",
        "memory.runtime_overhead=0",
        "",
    ])
    manifest_path = artifact_path + ".modelmanifest"
    with open(manifest_path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(content)
    return manifest_path

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--include-reference", action="store_true",
                        help="also select the host reference Transformers and GGUF artifacts")
    parser.add_argument("--with-embedding", action="store_true",
                        help="also select EmbeddingGemma 2 Text for retrieval tests")
    parser.add_argument("--verify-only", action="store_true",
                        help="check the selected local weights against pinned publisher SHA-256 values")
    args = parser.parse_args()
    roles = {"generation"}
    if args.with_embedding:
        roles.add("embedding")
    if args.include_reference:
        roles.add("reference")
    selected = [item for item in DOWNLOADS if item["role"] in roles]
    if args.verify_only:
        for item in selected:
            path = os.path.join(item["local_dir"], item["weights"])
            if not os.path.isfile(path):
                parser.exit(1, f"Missing model artifact: {path}\n")
            digest = sha256_file(path)
            if digest != item["sha256"]:
                parser.exit(1, f"SHA-256 mismatch for {path}: {digest}\n")
            print(f"Verified {path} ({os.path.getsize(path):,} bytes, SHA-256 {digest})", flush=True)
        return

    from huggingface_hub import hf_hub_download, HfApi

    api = HfApi()

    for item in selected:
        repo_id = item["repo_id"]
        local_dir = item["local_dir"]
        os.makedirs(local_dir, exist_ok=True)
        log(f"\n==================================================")
        log(f"Fetching repo metadata: {repo_id}")
        info = api.model_info(repo_id, revision=item["revision"])
        for filename in item["files"]:
            log(f"Downloading {repo_id}/{filename} -> {local_dir}...")
            t0 = time.time()
            local_path = hf_hub_download(
                repo_id=repo_id,
                filename=filename,
                revision=info.sha,
                local_dir=local_dir,
                force_download=False,
            )
            elapsed = time.time() - t0
            size = os.path.getsize(local_path)
            log(f"  Downloaded {filename}: {size} bytes ({size / (1024*1024):.2f} MB) in {elapsed:.2f}s")

            if not verify_not_lfs_pointer(local_path):
                log(f"  ERROR: {local_path} is a Git LFS pointer, not actual payload!")
                sys.exit(1)

            log(f"  Computing SHA-256 for {filename}...")
            digest = sha256_file(local_path)
            log(f"  SHA-256: {digest}")
            if filename == item["weights"] and digest != item["sha256"]:
                parser.exit(1, f"SHA-256 mismatch for {local_path}: expected {item['sha256']}\n")

            if filename == item["weights"]:
                manifest_path = write_model_manifest(item, local_path, size, digest)
                if manifest_path:
                    log(f"  Runtime manifest: {manifest_path}")

    log("\n[SUCCESS] Selected official artifacts acquired and verified successfully.")

if __name__ == "__main__":
    main()
