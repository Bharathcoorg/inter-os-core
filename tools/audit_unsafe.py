#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
InterOS Core — Unsafe Code Surface Diagnostic Tool

Audits the codebase for:
  - unsafe blocks
  - unsafe functions
  - unsafe impls
  - raw mutable statics (static mut)
  - extern interfaces
  - inline/global assembly blocks

Groups results by:
  - kernel
  - architecture
  - drivers
  - system services
  - SDK
  - native apps
  - AI
  - Android runtime
  - browser
"""

import os
import re
import sys

CATEGORY_DEFS = [
    ("architecture", lambda p: "kernel" in p and ("arch" in p or "interrupts" in p or "gdt" in p)),
    ("drivers", lambda p: "kernel" in p and ("driver" in p or "serial" in p or "framebuffer" in p or "rtc" in p or "audio" in p)),
    ("Android runtime", lambda p: "android" in p or "dex" in p or "apk" in p),
    ("browser", lambda p: "browser" in p or "webview" in p),
    ("AI", lambda p: ("rag" in p or "backend" in p or "embedding" in p or "gguf" in p or "weights" in p or "tokenizer" in p or "agent_manager" in p or "companion" in p or "appmcp" in p)),
    ("kernel", lambda p: p.startswith("kernel" + os.sep)),
    ("system services", lambda p: "system-service" in p or "init" in p),
    ("SDK", lambda p: "sdk" in p),
    ("native apps", lambda p: p.startswith("apps" + os.sep)),
]

def categorize(path):
    rel = os.path.relpath(path).replace("/", os.sep)
    for cat, predicate in CATEGORY_DEFS:
        if predicate(rel):
            return cat
    return "other"

RE_UNSAFE_FN = re.compile(r'\bunsafe\s+(?:extern\s+(?:"[^"]+"\s+)?)?fn\b')
RE_UNSAFE_IMPL = re.compile(r'\bunsafe\s+impl\b')
RE_UNSAFE_BLOCK = re.compile(r'\bunsafe\s*\{')
RE_STATIC_MUT = re.compile(r'\bstatic\s+mut\b')
RE_EXTERN_BLOCK = re.compile(r'\bextern\s+"[^"]+"\s*\{')
RE_ASM = re.compile(r'\b(?:core::arch::)?(?:asm|global_asm)!\s*\(')

class UnsafeMetric:
    def __init__(self):
        self.files = 0
        self.lines = 0
        self.unsafe_blocks = 0
        self.unsafe_fns = 0
        self.unsafe_impls = 0
        self.static_muts = 0
        self.extern_blocks = 0
        self.asm_blocks = 0

    @property
    def total(self):
        return (self.unsafe_blocks + self.unsafe_fns + self.unsafe_impls +
                self.static_muts + self.extern_blocks + self.asm_blocks)

def scan_file(filepath):
    try:
        with open(filepath, 'r', encoding='utf-8', errors='ignore') as f:
            content = f.read()
    except Exception:
        return None

    # Strip line comments and block comments for cleaner count
    cleaned = re.sub(r'//.*', '', content)
    cleaned = re.sub(r'/\*[\s\S]*?\*/', '', cleaned)

    lines = len(content.splitlines())
    u_blocks = len(RE_UNSAFE_BLOCK.findall(cleaned))
    u_fns = len(RE_UNSAFE_FN.findall(cleaned))
    u_impls = len(RE_UNSAFE_IMPL.findall(cleaned))
    s_muts = len(RE_STATIC_MUT.findall(cleaned))
    e_blocks = len(RE_EXTERN_BLOCK.findall(cleaned))
    asms = len(RE_ASM.findall(cleaned))

    return lines, u_blocks, u_fns, u_impls, s_muts, e_blocks, asms

def run_audit(root_dir="."):
    categories = {
        "kernel": UnsafeMetric(),
        "architecture": UnsafeMetric(),
        "drivers": UnsafeMetric(),
        "system services": UnsafeMetric(),
        "SDK": UnsafeMetric(),
        "native apps": UnsafeMetric(),
        "AI": UnsafeMetric(),
        "Android runtime": UnsafeMetric(),
        "browser": UnsafeMetric(),
        "other": UnsafeMetric(),
    }

    for dirpath, _, filenames in os.walk(root_dir):
        if "target" in dirpath or ".git" in dirpath or ".gemini" in dirpath:
            continue
        for fname in filenames:
            if fname.endswith(".rs"):
                fpath = os.path.join(dirpath, fname)
                cat = categorize(fpath)
                res = scan_file(fpath)
                if res is None:
                    continue
                lines, ub, uf, ui, sm, eb, ab = res
                m = categories[cat]
                m.files += 1
                m.lines += lines
                m.unsafe_blocks += ub
                m.unsafe_fns += uf
                m.unsafe_impls += ui
                m.static_muts += sm
                m.extern_blocks += eb
                m.asm_blocks += ab

    return categories

def print_report(categories):
    header = f"{'Subsystem / Category':<20} | {'Files':<5} | {'Lines':<8} | {'Blocks':<6} | {'Fns':<5} | {'Impls':<5} | {'StatMut':<7} | {'Extern':<6} | {'Asm':<5} | {'TOTAL':<6}"
    sep = "-" * len(header)
    print(header)
    print(sep)

    tot_files = tot_lines = tot_b = tot_fn = tot_i = tot_sm = tot_e = tot_a = tot_all = 0
    display_order = [
        "architecture",
        "drivers",
        "kernel",
        "system services",
        "SDK",
        "Android runtime",
        "AI",
        "browser",
        "native apps",
    ]

    for cat in display_order:
        m = categories.get(cat, UnsafeMetric())
        tot_files += m.files
        tot_lines += m.lines
        tot_b += m.unsafe_blocks
        tot_fn += m.unsafe_fns
        tot_i += m.unsafe_impls
        tot_sm += m.static_muts
        tot_e += m.extern_blocks
        tot_a += m.asm_blocks
        tot_all += m.total
        print(f"{cat:<20} | {m.files:<5} | {m.lines:<8} | {m.unsafe_blocks:<6} | {m.unsafe_fns:<5} | {m.unsafe_impls:<5} | {m.static_muts:<7} | {m.extern_blocks:<6} | {m.asm_blocks:<5} | {m.total:<6}")

    print(sep)
    print(f"{'OVERALL TOTAL':<20} | {tot_files:<5} | {tot_lines:<8} | {tot_b:<6} | {tot_fn:<5} | {tot_i:<5} | {tot_sm:<7} | {tot_e:<6} | {tot_a:<5} | {tot_all:<6}")

if __name__ == "__main__":
    cats = run_audit()
    print_report(cats)
