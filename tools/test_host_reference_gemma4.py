#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Host-side reference execution tool for official Gemma 4 E2B model checkpoints.

Executes genuine numerical inference using Google DeepMind's official weights
and reports full token sequences, latency, throughput, and memory consumption.
"""

import argparse
import os
import sys
import time
import torch
from transformers import AutoProcessor, Gemma4ForConditionalGeneration


def run_host_reference_inference(
    model_dir: str = "models/downloads/gemma4-mobile-qat",
    prompt_text: str = "Hello! Briefly explain why microkernels use capability security.",
    max_tokens: int = 40,
    enable_thinking: bool = False,
) -> dict:
    if not os.path.isdir(model_dir):
        raise FileNotFoundError(f"Model directory not found: {model_dir}")

    print(f"[*] Initializing AutoProcessor from {model_dir}...")
    t0_proc = time.perf_counter()
    processor = AutoProcessor.from_pretrained(model_dir)
    proc_load_time = time.perf_counter() - t0_proc
    print(f"    Processor initialized in {proc_load_time:.2f}s")

    print(f"[*] Loading Gemma4ForConditionalGeneration from {model_dir}...")
    t0_model = time.perf_counter()
    with torch.no_grad():
        model = Gemma4ForConditionalGeneration.from_pretrained(
            model_dir,
            torch_dtype=torch.float32,
            low_cpu_mem_usage=True,
        )
    model_load_time = time.perf_counter() - t0_model
    print(f"    Model weights loaded in {model_load_time:.2f}s")

    messages = [
        {"role": "user", "content": prompt_text},
    ]

    print("[*] Formatting prompt with chat template...")
    inputs = processor.apply_chat_template(
        messages,
        tokenize=True,
        add_generation_prompt=True,
        return_dict=True,
        return_tensors="pt",
        enable_thinking=enable_thinking,
    )

    input_ids = inputs["input_ids"]
    input_len = input_ids.shape[-1]
    prompt_tokens = input_ids[0].tolist()
    print(f"    Prompt tokens ({input_len} tokens): {prompt_tokens}")

    print(f"[*] Executing autoregressive generation (max_new_tokens={max_tokens}, do_sample=False)...")
    t0_gen = time.perf_counter()
    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=max_tokens,
            do_sample=False,
        )
    gen_time = time.perf_counter() - t0_gen

    output_seq = outputs[0].tolist()
    generated_tokens = output_seq[input_len:]
    gen_len = len(generated_tokens)
    tps = gen_len / gen_time if gen_time > 0 else 0.0

    decoded_text = processor.decode(generated_tokens, skip_special_tokens=True).strip()

    print("\n" + "=" * 60)
    print("GEMMA 4 E2B HOST REFERENCE INFERENCE RESULTS")
    print("=" * 60)
    print(f"Prompt:            {prompt_text}")
    print(f"Input Tokens:      {input_len}")
    print(f"Generated Tokens:  {gen_len}")
    print(f"Generation Time:   {gen_time:.2f}s ({tps:.2f} tokens/sec)")
    print(f"Output Tokens:     {generated_tokens}")
    print("-" * 60)
    print(f"Decoded Response:\n{decoded_text}")
    print("=" * 60 + "\n")

    return {
        "model_dir": model_dir,
        "prompt": prompt_text,
        "input_tokens": prompt_tokens,
        "input_len": input_len,
        "generated_tokens": generated_tokens,
        "gen_len": gen_len,
        "gen_time_sec": gen_time,
        "tokens_per_sec": tps,
        "decoded_response": decoded_text,
        "model_load_time_sec": model_load_time,
    }


def main():
    parser = argparse.ArgumentParser(description="Run host reference inference for Gemma 4 E2B.")
    parser.add_argument(
        "--model-dir",
        default="models/downloads/gemma4-mobile-qat",
        help="Path to the model directory.",
    )
    parser.add_argument(
        "--prompt",
        default="Hello! Briefly explain why microkernels use capability security.",
        help="Input user prompt.",
    )
    parser.add_argument(
        "--max-tokens",
        type=int,
        default=40,
        help="Maximum new tokens to generate.",
    )
    parser.add_argument(
        "--enable-thinking",
        action="store_true",
        help="Enable Gemma 4 thinking mode.",
    )
    args = parser.parse_args()

    run_host_reference_inference(
        model_dir=args.model_dir,
        prompt_text=args.prompt,
        max_tokens=args.max_tokens,
        enable_thinking=args.enable_thinking,
    )


if __name__ == "__main__":
    main()
