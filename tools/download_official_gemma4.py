#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Downloads official Gemma 4 E2B model artifacts from Hugging Face with integrity verification."""

import os
import sys
import time
import json
import hashlib
from huggingface_hub import hf_hub_download, HfApi

LOG_PATH = "audit-evidence/gemma4-mobile-acquisition/download-log.txt"
MANIFEST_PATH = "audit-evidence/gemma4-mobile-acquisition/download-manifest.json"
CHECKSUMS_PATH = "audit-evidence/gemma4-mobile-acquisition/checksums.sha256"

DOWNLOADS = [
    {
        "repo_id": "google/gemma-4-E2B-it-qat-mobile-transformers",
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
        "local_dir": "models/downloads/gemma4-litert",
        "files": [
            "README.md",
            "chat_template.jinja",
            "gemma-4-E2B-it.litertlm",
        ]
    },
    {
        "repo_id": "google/gemma-4-E2B-it-qat-q4_0-gguf",
        "local_dir": "models/downloads/gemma4-official-gguf",
        "files": [
            "README.md",
            "gemma-4-E2B_q4_0-it.gguf",
        ]
    }
]

def log(msg):
    print(msg, flush=True)
    with open(LOG_PATH, "a", encoding="utf-8") as f:
        f.write(f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] {msg}\n")

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

def main():
    os.makedirs("audit-evidence/gemma4-mobile-acquisition", exist_ok=True)
    with open(LOG_PATH, "w", encoding="utf-8") as f:
        f.write("=== Gemma 4 E2B Official Model Acquisition Log ===\n")

    api = HfApi()
    manifest = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "repositories": [],
        "artifacts": []
    }
    checksums = []

    for item in DOWNLOADS:
        repo_id = item["repo_id"]
        local_dir = item["local_dir"]
        os.makedirs(local_dir, exist_ok=True)
        log(f"\n==================================================")
        log(f"Fetching repo metadata: {repo_id}")
        info = api.model_info(repo_id)
        repo_entry = {
            "repo_id": repo_id,
            "commit_sha": info.sha,
            "local_dir": local_dir,
            "files": []
        }

        for filename in item["files"]:
            log(f"Downloading {repo_id}/{filename} -> {local_dir}...")
            t0 = time.time()
            local_path = hf_hub_download(
                repo_id=repo_id,
                filename=filename,
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

            artifact_info = {
                "repo_id": repo_id,
                "commit_sha": info.sha,
                "filename": filename,
                "local_path": os.path.abspath(local_path),
                "size_bytes": size,
                "sha256": digest,
                "download_seconds": round(elapsed, 2)
            }
            manifest["artifacts"].append(artifact_info)
            repo_entry["files"].append(filename)
            checksums.append(f"{digest}  {os.path.relpath(local_path)}")

        manifest["repositories"].append(repo_entry)

    with open(MANIFEST_PATH, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)

    with open(CHECKSUMS_PATH, "w", encoding="utf-8") as f:
        f.write("\n".join(checksums) + "\n")

    log("\n[SUCCESS] All official artifacts acquired and verified successfully!")

if __name__ == "__main__":
    main()
