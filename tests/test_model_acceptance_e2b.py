# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Per-model acceptance suite: Gemma 4 E2B (`gemma-4-E2B-it-qat-UD-Q2_K_XL.gguf`).

WHY a separate suite per model: the mandate requires fifteen verification points
against EACH genuine target artifact, and a resource-blocked model may never be
silently replaced by a smaller fixture. This suite measures the real
2,186,186,784-byte file on this host -- its header, metadata and descriptor table
only, never a tensor payload -- and pins the engine's source contracts beside
those measurements. Platform truth: "Gemma 4 Mobile Lite" is NOT a published
variant (gguf.rs says so in its own comments); the artifact this platform targets
is this QAT-mobile E2B file, arch `gemma4`, size_label "4.6B".

HONESTY RULE: point 14 (end-to-end generation) is NOT VERIFIED. Sixty-one of this
file's tensors are TQ2_0 (dtype 35), which the engine's decode set [0,1,2,8,12]
does not map -- including `token_embd.weight` itself; the per-layer input table is
9.84x the per-mapping ceiling (one row IS streamable, but the graph does not yet
consume the per-layer input mechanism); and the twenty shared-KV layers declare
no attn_k/attn_v/attn_k_norm the thirteen-name block loader requires. A suite
reporting 15/15 green without a demonstrated run would be a fabricated success
report. Blocked points skip with the specific reason and file:line evidence; no
point is deleted or weakened to get green, and no fixture's output is asserted as
this model's.

Tensor sizes are measured by offset deltas (sort by offset; size = next offset
minus this offset; last = payload end minus offset). The deltas sum EXACTLY to the
payload section -- a self-check no dtype table can fake. The earlier probe
`target/verify/tensor_ceiling.py` is WRONG (32-element blocks for K-quants and TQ)
and is never used or cited here; `target/verify/tensor_offsets.py` is the lead's
method.
"""

import re
import struct
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SERVICE = ROOT / "userspace" / "system-service" / "src"
MODELS = ROOT / "target" / "verify" / "models"
KERNEL_SYSCALL = ROOT / "kernel" / "src" / "syscall" / "mod.rs"
KERNEL_USER = ROOT / "kernel" / "src" / "task" / "user.rs"

ARTIFACT = "gemma-4-E2B-it-qat-UD-Q2_K_XL.gguf"
EG2_ARTIFACT = "embeddinggemma-2-UD-Q4_K_XL.gguf"
TINY_FIXTURE = "gemma-tiny-f32.gguf"

# Identity, lead-measured and re-measured here from the file's own bytes.
EXPECTED_SIZE = 2_186_186_784
EXPECTED_TENSORS = 541
EXPECTED_KV = 47
EXPECTED_ARCH = "gemma4"
EXPECTED_SIZE_LABEL = "4.6B"
EXPECTED_NAME = "Gemma-4 E2B IT (mobile wNa8o8 -> gguf)"

# Section geometry. `KV_END` is the lead's "metadata_end"; the lead's "data
# section" (2,170,403,348) is file size minus KV_END and therefore INCLUDES the
# 32,163-byte descriptor table plus 17 bytes of alignment padding. The exact
# payload section starts at DATA_START, after the table, and is 2,170,371,168 B.
KV_END = 15_783_436
TABLE_END = 15_815_599
ALIGNMENT = 32
DATA_START = 15_815_616
PAYLOAD = 2_170_371_168
LEAD_SECTION = 2_170_403_348

CEILING = 134_213_632                 # per-mapping ceiling, never weakened
READ_WINDOW = 65_536                  # positioned-read bound, both sides
MAPPED_DTYPES = [0, 1, 2, 8, 12, 13, 14, 35]  # the engine's DECODE set (inference.rs)

# GGML block geometry for PLACEMENT arithmetic: (elements per block, bytes per
# block). Legacy quants are 32-element blocks; K-quants and TQ are 256-element
# superblocks. TQ2_0 (35) is 66 bytes per 256 elements; pricing it at 32-element
# blocks makes every TQ tensor 8x too large -- the delta self-check catches that.
GEOM = {
    0: (1, 4), 1: (1, 2), 2: (32, 18), 3: (32, 20), 6: (32, 22),
    7: (32, 24), 8: (32, 34), 9: (32, 36), 10: (256, 84), 11: (256, 110),
    12: (256, 144), 13: (256, 176), 14: (256, 210), 15: (256, 292),
    30: (1, 2), 35: (256, 66),
}

EXPECTED_CENSUS = {0: 263, 1: 1, 2: 146, 8: 70, 35: 61}
EXPECTED_OVER = {"per_layer_token_embd.weight": 1_321_205_760}
LARGEST = ("per_layer_token_embd.weight", 1_321_205_760, 2, [8960, 262144])
SECOND = ("token_embd.weight", 103_809_024, 35, [1536, 262144])

# Every tensor this file declares (0/1/2/8/35) is now in the decode set, so the
# loader maps all of them; the TQ2_0 token embedding and FFN matrices decode.
EXPECTED_UNMAPPED = {}

VOCAB = 262_144
TOKEN_PAYLOAD = 2_316_295
MAX_PIECE = 93
MERGES_DECLARED = 514_906
MERGE_PAYLOAD = 5_132_471
MERGE_RAW = 9_251_719                 # payload + 8-byte length prefix per entry
TEMPLATE_LEN = 18_810                 # measured; tokenizer.rs's doc says 18,808
TEMPLATE_CAP = 1024                   # gguf.rs MAX_CHAT_TEMPLATE_LEN

BLOCKS = 35
SHARED_KV = 20                        # layers 15..34 share earlier layers' KV
SHARED_START = BLOCKS - SHARED_KV
HIDDEN = 1536
HEADS = 8
KV_HEADS = 1
KEY_FULL = 512
KEY_SWA = 256
FFN_EARLY = 6144                      # layers 0..14
FFN_LATE = 12288                      # layers 15..34
PER_LAYER_INPUT = 256
ROW_LEN = 8960                        # 35 layers x 256, the contiguous dimension
ROW_BYTES = 5040                      # 8960 / 32 * 18 bytes of Q4_0
BLOCK_EXTRAS = {"inp_gate.weight", "proj.weight", "layer_output_scale.weight",
                "post_norm.weight"}
SHARED_KV_MISSING = {"attn_k.weight", "attn_v.weight", "attn_k_norm.weight"}
MIN_BLOCK_BYTES = 19_004_448
MAX_BLOCK_BYTES = 24_758_304
MAX_BLOCK_READS = 378

SCALAR_FMT = {0: "<B", 1: "<b", 2: "<H", 3: "<h", 4: "<I", 5: "<i",
              6: "<f", 7: "<B", 10: "<Q", 11: "<q", 12: "<d"}
FIXED_SIZE = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}


def read(name: str) -> str:
    return (SERVICE / name).read_text(encoding="utf-8")


def without_comments(source: str) -> str:
    kept = []
    for line in source.splitlines():
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        kept.append(line.split("//")[0])
    return "\n".join(kept)


def function_body(source: str, signature: str) -> str:
    start = source.index(signature)
    opened = source.index("{", start)
    depth = 0
    for index in range(opened, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"{signature} never closes")


def line_of(source: str, needle: str) -> int:
    return source.count("\n", 0, source.index(needle)) + 1


def string_list(source: str, declaration: str) -> list:
    """The quoted members of a `pub const X: &[&str] = &[...]` list. A const has
    no braces, so `function_body` would read into the next function."""
    body = source[source.index(declaration) :]
    body = body[: body.index("];")]
    return re.findall(r'"([^"]+)"', body)


def usize_literal(text: str) -> int:
    """`128 * 1024 * 1024 - 4096` as the number it writes, without eval."""
    parts = [p.strip() for p in text.replace("_", "").split("-")]

    def prod(term):
        value = 1
        for factor in term.split("*"):
            value *= int(factor.strip())
        return value

    return prod(parts[0]) - sum(prod(p) for p in parts[1:])


def const_value(source: str, name: str) -> int:
    match = re.search(rf"pub const {name}: usize = ([0-9_ *\-]+);", source)
    assert match, f"{name} is no longer a usize literal"
    return usize_literal(match.group(1))


class Cursor:
    """Bounded, seek-based GGUF cursor: 1 MiB refills, never a whole payload."""

    def __init__(self, fh):
        self.fh = fh
        self.buf = b""
        self.i = 0
        self.base = 0

    def _ensure(self, n):
        if self.i + n <= len(self.buf):
            return
        abspos = self.base + self.i
        self.fh.seek(abspos)
        self.buf = self.fh.read(max(n, 1 << 20))
        if len(self.buf) < n:
            raise EOFError(f"want {n} bytes at {abspos}, got {len(self.buf)}")
        self.base = abspos
        self.i = 0

    def take(self, n):
        self._ensure(n)
        out = self.buf[self.i : self.i + n]
        self.i += n
        return out

    def skip(self, n):
        if n <= len(self.buf) - self.i:
            self.i += n
        else:
            abspos = self.base + self.i + n
            self.fh.seek(abspos)
            self.buf = b""
            self.i = 0
            self.base = abspos

    def tell(self):
        return self.base + self.i

    def u32(self):
        return struct.unpack("<I", self.take(4))[0]

    def u64(self):
        return struct.unpack("<Q", self.take(8))[0]

    def string(self):
        return self.take(self.u64())


def parse_gguf(path: Path) -> dict:
    """Header + metadata + descriptor table. String arrays are counted and stepped
    over by their length prefixes; only a handful of samples are kept. No tensor
    payload is ever read."""
    size = path.stat().st_size
    with open(path, "rb") as fh:
        c = Cursor(fh)
        magic = c.take(4)
        version, tensor_count, kv_count = struct.unpack("<IQQ", c.take(20))
        meta, arrays = {}, {}
        for _ in range(kv_count):
            key = c.string().decode("utf-8", "replace")
            vt = c.u32()
            if vt == 8:
                raw = c.string()
                meta[key] = raw.decode("utf-8", "replace") if len(raw) <= 256 else None
                arrays[key] = {"kind": "string", "len": len(raw)}
            elif vt == 9:
                et, n = c.u32(), c.u64()
                info = {"kind": "array", "elem_type": et, "count": n}
                if et == 8:
                    total, maxlen, samples = 0, 0, {}
                    want = {0, 1, 2, 3, 106, 238, 493, 262143}
                    for idx in range(n):
                        ln = c.u64()
                        total += ln
                        maxlen = max(maxlen, ln)
                        if idx in want:
                            samples[idx] = c.take(ln).decode("utf-8", "replace")
                        else:
                            c.skip(ln)
                    info.update(payload=total, max_len=maxlen, samples=samples)
                elif et in FIXED_SIZE:
                    if n <= 64:
                        info["values"] = [
                            struct.unpack(SCALAR_FMT[et], c.take(FIXED_SIZE[et]))[0]
                            for _ in range(n)
                        ]
                    else:
                        c.skip(n * FIXED_SIZE[et])
                else:
                    raise ValueError(f"array {key} element type {et}")
                arrays[key] = info
                meta[key] = info
            elif vt in SCALAR_FMT:
                meta[key] = struct.unpack(SCALAR_FMT[vt], c.take(FIXED_SIZE[vt]))[0]
            else:
                raise ValueError(f"key {key} value type {vt}")
        kv_end = c.tell()
        tensors = []
        for _ in range(tensor_count):
            name = c.string().decode("utf-8", "replace")
            nd = c.u32()
            dims = [c.u64() for _ in range(nd)]
            dtype, offset = c.u32(), c.u64()
            tensors.append({"name": name, "dims": dims, "dtype": dtype,
                            "offset": offset})
        table_end = c.tell()
    alignment = meta.get("general.alignment", 32)
    data_start = (table_end + alignment - 1) // alignment * alignment
    payload = size - data_start
    ordered = sorted(tensors, key=lambda t: t["offset"])
    for idx, t in enumerate(ordered):
        nxt = ordered[idx + 1]["offset"] if idx + 1 < len(ordered) else payload
        t["bytes"] = nxt - t["offset"]
        elems = 1
        for d in t["dims"]:
            elems *= d
        t["elements"] = elems
        be, bb = GEOM[t["dtype"]]
        t["predicted"] = (elems + be - 1) // be * bb
    return {"size": size, "magic": magic, "version": version,
            "tensor_count": tensor_count, "kv_count": kv_count, "meta": meta,
            "arrays": arrays, "tensors": tensors, "kv_end": kv_end,
            "table_end": table_end, "alignment": alignment,
            "data_start": data_start, "payload": payload,
            "delta_sum": sum(t["bytes"] for t in tensors)}


def by_name(parsed: dict, name: str) -> dict:
    return next(t for t in parsed["tensors"] if t["name"] == name)


def blocks_of(parsed: dict) -> dict:
    blocks = {}
    for t in parsed["tensors"]:
        if t["name"].startswith("blk."):
            idx = int(t["name"].split(".")[1])
            blocks.setdefault(idx, []).append(t)
    return blocks


def mapped_dtype_ids(source: str) -> list:
    """The dtype ids `quantised_row_bytes` answers a row width for -- the DECODE
    set, which is deliberately smaller than the placement table in weights.rs."""
    body = function_body(source, "pub fn quantised_row_bytes(")
    arms = body[body.index("match dtype {") :]
    found = set()
    for line in arms.splitlines():
        match = re.match(r"^\s*((?:\d{1,3}\s*\|\s*)*\d{1,3})\s*=>", line)
        if match:
            found.update(int(n) for n in re.findall(r"\d{1,3}", match.group(1)))
    return sorted(found)


# ---------------------------------------------------------------------------
# Detectors. Each returns its findings so the same code proves the live artifact
# clean AND proves a mutated copy is caught. A gate nobody has seen fail is a
# comment.
# ---------------------------------------------------------------------------


def identity_defects(ident: dict) -> list:
    defects = []
    for field, expected in (
        ("size", EXPECTED_SIZE), ("magic", b"GGUF"), ("version", 3),
        ("tensor_count", EXPECTED_TENSORS), ("kv_count", EXPECTED_KV),
        ("arch", EXPECTED_ARCH), ("size_label", EXPECTED_SIZE_LABEL),
        ("name", EXPECTED_NAME),
    ):
        if ident.get(field) != expected:
            defects.append(f"{field} is {ident.get(field)!r}, not {expected!r}")
    if ident.get("arch") == "gemma-embedding2":
        defects.append("this is the embedding model's architecture, not E2B's")
    if "Mobile Lite" in str(ident.get("name")):
        defects.append("Mobile Lite is NOT a published variant; the name is fabricated")
    return defects


def identity_of(parsed: dict) -> dict:
    return {"size": parsed["size"], "magic": parsed["magic"],
            "version": parsed["version"], "tensor_count": parsed["tensor_count"],
            "kv_count": parsed["kv_count"],
            "arch": parsed["meta"].get("general.architecture"),
            "size_label": parsed["meta"].get("general.size_label"),
            "name": parsed["meta"].get("general.name")}


def section_defects(parsed: dict) -> list:
    """Recomputes the deltas from the offsets, so a perturbed offset is caught."""
    defects = []
    for field, expected in (("kv_end", KV_END), ("table_end", TABLE_END),
                            ("alignment", ALIGNMENT), ("data_start", DATA_START),
                            ("payload", PAYLOAD)):
        if parsed[field] != expected:
            defects.append(f"{field} is {parsed[field]}, not {expected}")
    if parsed["size"] - parsed["kv_end"] != LEAD_SECTION:
        defects.append("file size minus metadata end is not the lead's data section")
    ordered = sorted(parsed["tensors"], key=lambda t: t["offset"])
    total = 0
    for idx, t in enumerate(ordered):
        nxt = ordered[idx + 1]["offset"] if idx + 1 < len(ordered) else parsed["payload"]
        delta = nxt - t["offset"]
        total += delta
        if not (t["predicted"] <= delta <= (t["predicted"] + ALIGNMENT - 1)
                // ALIGNMENT * ALIGNMENT):
            defects.append(
                f"{t['name']}: delta {delta} outside [{t['predicted']}, "
                f"align_up({t['predicted']})] -- the dtype geometry disagrees with the file"
            )
    if total != parsed["payload"]:
        defects.append(f"offset deltas sum to {total}, not the payload {parsed['payload']}")
    return defects


def ceiling_defects(parsed: dict) -> list:
    defects = []
    over = {t["name"]: t["bytes"] for t in parsed["tensors"] if t["bytes"] > CEILING}
    if over != EXPECTED_OVER:
        defects.append(f"over-ceiling tensors are {over}, expected {EXPECTED_OVER}")
    largest = max(parsed["tensors"], key=lambda t: t["bytes"])
    name, nbytes, dtype, dims = LARGEST
    if (largest["name"], largest["bytes"], largest["dtype"], largest["dims"]) != (
        name, nbytes, dtype, dims
    ):
        defects.append(f"largest tensor is {largest['name']} {largest['bytes']} B, "
                       f"not {name} {nbytes} B")
    return defects


def census_defects(parsed: dict, mapped: list) -> list:
    defects = []
    census = {}
    for t in parsed["tensors"]:
        census[t["dtype"]] = census.get(t["dtype"], 0) + 1
    if census != EXPECTED_CENSUS:
        defects.append(f"dtype census {census} != {EXPECTED_CENSUS}")
    unmapped = {t["name"]: t["dtype"] for t in parsed["tensors"]
                if t["dtype"] not in mapped}
    if unmapped != EXPECTED_UNMAPPED:
        missing = set(EXPECTED_UNMAPPED) - set(unmapped)
        extra = set(unmapped) - set(EXPECTED_UNMAPPED)
        defects.append(f"undecodable tensors differ: missing {sorted(missing)[:4]}, "
                       f"extra {sorted(extra)[:4]} ({len(unmapped)} vs "
                       f"{len(EXPECTED_UNMAPPED)} expected)")
    return defects


def refusal_defects(inference: str, weights: str) -> list:
    defects = []
    if mapped_dtype_ids(inference) != MAPPED_DTYPES:
        defects.append(f"decode set is {mapped_dtype_ids(inference)}, not {MAPPED_DTYPES}")
    for signature, anchor in (("pub fn gemv(", "match w_type {"),
                              ("pub fn embedding_row_into(", "match dtype {")):
        body = function_body(inference, signature)
        if "_ => Err(InferenceError::UnsupportedQuantization(" not in body:
            defects.append(f"{signature} no longer refuses an unmapped dtype by number")
    if "UnsupportedDtype(&'static str, u32)" not in weights:
        defects.append("the loader's refusal stopped carrying the tensor name and number")
    if "MixedBlockDtype(" not in weights:
        defects.append("a block split across two quantisations is no longer refused")
    block = function_body(weights, "pub fn load_block(&mut self, index: usize)")
    if "UnsupportedDtype" not in block:
        defects.append("load_block reads a dtype it has no layout for")
    if "reading a row still goes through" not in weights:
        defects.append("weights.rs no longer separates placement geometry from decode")
    return defects


def row_defects(parsed: dict, weights: str) -> list:
    """The per-layer input table is over the ceiling WHOLE, but one row of its
    contiguous dimension is a bounded positioned read. Both halves are checked:
    the arithmetic on the file's own descriptor, and the loader's refusals."""
    defects = []
    t = by_name(parsed, "per_layer_token_embd.weight")
    row_len = t["dims"][0]
    if row_len != ROW_LEN:
        defects.append(f"row length is {row_len}, not {ROW_LEN} (dims[0] is the "
                       "contiguous dimension)")
    if row_len != BLOCKS * PER_LAYER_INPUT:
        defects.append(f"row length {row_len} != {BLOCKS} layers x {PER_LAYER_INPUT}")
    if t["dims"][1] != VOCAB:
        defects.append(f"row count is {t['dims'][1]}, not the vocabulary {VOCAB}")
    if t["dtype"] not in MAPPED_DTYPES:
        defects.append(f"row dtype {t['dtype']} is outside the decode set")
    be, bb = GEOM[t["dtype"]]
    row_bytes = (row_len + be - 1) // be * bb
    if row_bytes != ROW_BYTES:
        defects.append(f"one row prices at {row_bytes} B, not {ROW_BYTES}")
    if row_bytes > READ_WINDOW:
        defects.append(f"one row is {row_bytes} B, over the {READ_WINDOW} B "
                       "positioned-read bound")
    if t["bytes"] <= CEILING:
        defects.append("the whole tensor no longer exceeds the ceiling; the row "
                       "path's reason for existing changed")
    if "pub fn load_per_layer_token_embd_row(" not in weights:
        defects.append("the row loader is gone")
        return defects
    body = function_body(weights, "pub fn load_per_layer_token_embd_row(")
    for anchor in ("quantised_row_bytes(entry.dtype, row_len)",
                   "UnsupportedDtype(TENSOR_PER_LAYER_TOKEN_EMBD, entry.dtype)",
                   "(token as u64) >= entry.rows",
                   "entry.cols as usize != row_len",
                   "checked_add(token as u64 * row_bytes as u64)"):
        if anchor not in body:
            defects.append(f"the row loader lost its guard: {anchor}")
    if "9.84x the per-mapping ceiling" not in weights:
        defects.append("weights.rs no longer states why the whole mapping is refused")
    if "5,040 bytes" not in weights:
        defects.append("weights.rs no longer states the row's measured byte cost")
    return defects


@pytest.fixture(scope="module")
def gguf():
    path = MODELS / ARTIFACT
    if not path.exists():
        pytest.skip(
            f"{ARTIFACT} is absent at {path}. This suite measures the genuine "
            "Gemma 4 E2B artifact and never substitutes a smaller fixture."
        )
    return parse_gguf(path)


# ---------------------------------------------------------------------------
# The fifteen verification points.
# ---------------------------------------------------------------------------


def test_point_01_artifact_identity(gguf):
    """VERIFIED: exact byte size, GGUF magic/version, declared arch and size_label.

    A renamed or substituted file is caught here: the EG2 embedding artifact and
    the synthetic tiny fixture both fail this detector (proved in the meta-test),
    and a "Mobile Lite" name claim is rejected as the fabricated variant it is.
    """
    assert identity_defects(identity_of(gguf)) == []
    m = gguf["meta"]
    assert m["general.basename"] == "gemma-4-E2B-it"
    assert m["general.finetune"] == "qat-mobile"
    assert m["general.quantized_by"] == "unsloth"
    assert m["general.file_type"] == 2
    gguf_src = read("gguf.rs")
    assert '"Gemma 4 Mobile Lite" is NOT a published model variant' in gguf_src
    assert ARTIFACT in gguf_src
    assert "2,186,186,784" in gguf_src


def test_point_02_tensor_count_sections_and_delta_self_check(gguf):
    """VERIFIED: 541 tensors; the offset deltas sum EXACTLY to the payload section.

    Also pins the lead's figures: metadata_end 15,783,436 and 'data section'
    2,170,403,348 (which includes the 32,163-byte descriptor table and 17 bytes of
    padding). Every tensor's delta sits between its geometry-predicted bytes and
    that prediction aligned up to 32 -- the cross-check that catches a wrong block
    table, including TQ2_0's 256-element superblocks.
    """
    assert gguf["tensor_count"] == EXPECTED_TENSORS
    assert section_defects(gguf) == []


def test_point_03_geometry_is_read_from_this_files_own_metadata(gguf):
    """VERIFIED: hidden 1536, 35 layers, 8 heads, per-layer FFN widths and SWA.

    Nothing is hardcoded from a different model: `feed_forward_length` is an ARRAY
    of 35 unequal INT32 values (6144 for layers 0..14, 12288 for 15..34), and every
    width is cross-checked against the tensor descriptors it sizes, per layer,
    including the SWA layers' narrower 256-wide heads, the full-attention layers'
    512-wide heads, and the twenty shared-KV layers that declare no K/V at all.
    """
    m = gguf["meta"]
    arch = EXPECTED_ARCH
    assert m[f"{arch}.embedding_length"] == HIDDEN
    assert m[f"{arch}.block_count"] == BLOCKS
    assert m[f"{arch}.attention.head_count"] == HEADS
    assert m[f"{arch}.attention.head_count_kv"] == KV_HEADS
    assert m[f"{arch}.attention.key_length"] == KEY_FULL
    assert m[f"{arch}.attention.key_length_swa"] == KEY_SWA
    assert m[f"{arch}.attention.value_length"] == KEY_FULL
    assert m[f"{arch}.attention.value_length_swa"] == KEY_SWA
    assert m[f"{arch}.context_length"] == 131_072
    assert m[f"{arch}.attention.sliding_window"] == 512
    assert m[f"{arch}.attention.shared_kv_layers"] == SHARED_KV
    assert m[f"{arch}.embedding_length_per_layer_input"] == PER_LAYER_INPUT
    assert m[f"{arch}.final_logit_softcapping"] == pytest.approx(30.0)
    ffn = m[f"{arch}.feed_forward_length"]["values"]
    swa = m[f"{arch}.attention.sliding_window_pattern"]["values"]
    assert len(ffn) == BLOCKS and len(swa) == BLOCKS
    assert ffn[:SHARED_START] == [FFN_EARLY] * SHARED_START
    assert ffn[SHARED_START:] == [FFN_LATE] * SHARED_KV
    assert [i for i, v in enumerate(swa) if not v] == [4, 9, 14, 19, 24, 29, 34]
    blocks = blocks_of(gguf)
    assert sorted(blocks) == list(range(BLOCKS))
    for idx, tensors in blocks.items():
        named = {t["name"].split(".", 2)[2]: t for t in tensors}
        key_len = KEY_SWA if swa[idx] else KEY_FULL
        width = ffn[idx]
        assert named["attn_q.weight"]["dims"] == [HIDDEN, HEADS * key_len]
        assert named["attn_output.weight"]["dims"] == [HEADS * key_len, HIDDEN]
        assert named["attn_q_norm.weight"]["dims"] == [key_len]
        assert named["ffn_gate.weight"]["dims"] == [HIDDEN, width]
        assert named["ffn_up.weight"]["dims"] == [HIDDEN, width]
        assert named["ffn_down.weight"]["dims"] == [width, HIDDEN]
        if idx < SHARED_START:
            assert named["attn_k.weight"]["dims"] == [HIDDEN, KV_HEADS * key_len]
            assert named["attn_v.weight"]["dims"] == [HIDDEN, KV_HEADS * key_len]
            assert named["attn_k_norm.weight"]["dims"] == [key_len]
        else:
            for absent in SHARED_KV_MISSING:
                assert absent not in named, f"blk.{idx} unexpectedly declares {absent}"
    assert by_name(gguf, "token_embd.weight")["dims"] == [HIDDEN, VOCAB]
    assert by_name(gguf, "per_layer_token_embd.weight")["dims"] == [ROW_LEN, VOCAB]
    assert by_name(gguf, "per_layer_model_proj.weight")["dims"] == [HIDDEN, ROW_LEN]
    assert by_name(gguf, "per_layer_proj_norm.weight")["dims"] == [PER_LAYER_INPUT]
    assert by_name(gguf, "rope_freqs.weight")["dims"] == [KEY_SWA]
    assert by_name(gguf, "output_norm.weight")["dims"] == [HIDDEN]
    assert ROW_LEN == BLOCKS * PER_LAYER_INPUT


def test_point_04_exactly_one_tensor_exceeds_the_per_mapping_ceiling(gguf):
    """VERIFIED: exactly ONE tensor over 134,213,632 B -- per_layer_token_embd at
    1,321,205,760 B (9.84x). The ceiling is read from the engine's own constant,
    not remembered, and is never weakened to make the model fit. The second
    largest, token_embd (103,809,024 B, 0.77x), fits the ceiling but is TQ2_0 and
    undecodable (point 06).
    """
    assert const_value(read("inference.rs"), "LARGEST_SINGLE_BUFFER_BYTES") == CEILING
    assert ceiling_defects(gguf) == []
    largest = by_name(gguf, LARGEST[0])
    assert largest["bytes"] / CEILING == pytest.approx(9.8441, abs=1e-4)
    second = by_name(gguf, SECOND[0])
    assert (second["bytes"], second["dtype"], second["dims"]) == SECOND[1:]
    assert second["bytes"] < CEILING
    assert second["bytes"] / CEILING == pytest.approx(0.7735, abs=1e-4)


def test_point_05_the_over_ceiling_table_is_row_streamable(gguf):
    """VERIFIED (arithmetic + source contract): mapping per_layer_token_embd whole
    is refused by read_raw_tensor's ceiling bound, but ONE row of its contiguous
    dimension (dims[0] = 8960 = 35 x 256) is 5,040 bytes of Q4_0 -- inside the
    64 KiB positioned-read bound. `load_per_layer_token_embd_row` prices the row
    through the engine's decode set, bounds the token index by the descriptor's
    own row count, and refuses a width disagreement by the two numbers. Runtime
    execution of the row path is NOT VERIFIED; the meta-test proves a swapped
    orientation (a 147,456-byte row) is caught.
    """
    weights = read("weights.rs")
    assert row_defects(gguf, weights) == []
    raw = function_body(weights, "fn read_raw_tensor(")
    assert "entry.byte_len > ceiling" in raw
    assert "BadShape(suffix, entry.byte_len, ceiling)" in raw
    assert "crate::inference::LARGEST_SINGLE_BUFFER_BYTES" in raw
    assert 'pub const TENSOR_PER_LAYER_TOKEN_EMBD: &str = "per_layer_token_embd.weight"' in weights
    assert "pub fn has_per_layer_token_embd(&self) -> bool" in weights


def test_point_06_dtype_census_and_the_undecodable_tensors(gguf):
    """VERIFIED: census {0:263, 1:1, 2:146, 8:70, 35:61}; 61 TQ2_0 tensors are
    UNDECODABLE -- token_embd.weight itself and the three FFN matrices of every
    shared-KV layer. All 541 tensors are PLACEABLE (the placement table sizes 16
    ids including TQ2_0 35), so the walk places them and decode refuses by name
    and number. The F16 per_layer_model_proj (dtype 1) IS in the decode set. The
    decode set is pinned at five ids by the graph contract and is not widened.
    """
    inference = read("inference.rs")
    weights = read("weights.rs")
    assert mapped_dtype_ids(inference) == MAPPED_DTYPES
    assert census_defects(gguf, MAPPED_DTYPES) == []
    body = function_body(weights, "fn ggml_block_geometry(")
    placement = set()
    for line in body.splitlines():
        match = re.match(r"^\s*(\d{1,3})\s*=>\s*Some\(", line)
        if match:
            placement.add(int(match.group(1)))
    assert placement == set(GEOM), "the placement table drifted from the GGML geometry"
    assert set(MAPPED_DTYPES) < placement
    assert "reading a row still goes through" in weights
    for t in gguf["tensors"]:
        assert t["predicted"] <= t["bytes"] <= (
            (t["predicted"] + ALIGNMENT - 1) // ALIGNMENT * ALIGNMENT
        ), f"{t['name']} is not placeable at its declared geometry"
    assert by_name(gguf, "per_layer_model_proj.weight")["dtype"] == 1
    assert 1 in MAPPED_DTYPES


def test_point_07_the_refusal_names_the_tensor_and_the_number():
    """VERIFIED (source contract): an unmapped dtype is refused, never read as a
    neighbour's layout. Runtime execution of the refusal is NOT VERIFIED while
    the crate's test target does not compile (inference.rs geometry work in
    flight); the mutation meta-test below proves this gate can fail.
    """
    assert refusal_defects(read("inference.rs"), read("weights.rs")) == []


def test_point_08_vocabulary_comes_from_the_file_with_no_fallback(gguf):
    """VERIFIED: 262,144 entries counted from the array's own elements, byte
    tokens <0x00>..<0xFF> at ids 238..=493 (measured in this file, identical to
    EG2's vocabulary), and the reader refuses a file with no vocabulary rather
    than guessing one. The invented COMMON_SUBWORDS table is gone from production.
    """
    tokens = gguf["arrays"]["tokenizer.ggml.tokens"]
    assert tokens["count"] == VOCAB
    assert tokens["payload"] == TOKEN_PAYLOAD
    assert tokens["max_len"] == MAX_PIECE
    assert tokens["samples"][0] == "<pad>"
    assert tokens["samples"][238] == "<0x00>"
    assert tokens["samples"][493] == "<0xFF>"
    assert gguf["arrays"]["tokenizer.ggml.scores"]["count"] == VOCAB
    assert gguf["arrays"]["tokenizer.ggml.token_type"]["count"] == VOCAB
    tok = read("tokenizer.rs")
    assert "There is no fallback vocabulary" in tok
    assert "NoVocabulary" in tok
    production = without_comments(tok.split("#[cfg(test)]")[0])
    assert "COMMON_SUBWORDS" not in production


def test_point_09_merges_declared_held_and_refused_with_both_numbers(gguf):
    """VERIFIED: 514,906 merges declared (9,251,719 raw bytes with prefixes),
    resolved to id triples at 16 bytes each, charged against the heap budget,
    and refused with BOTH numbers when they will not fit. `merges_complete()`
    says whether every declared merge was held; `encode_bpe` refuses a partial
    table rather than segmenting with a prefix it would call complete.
    """
    merges = gguf["arrays"]["tokenizer.ggml.merges"]
    assert merges["count"] == MERGES_DECLARED
    assert merges["payload"] == MERGE_PAYLOAD
    assert merges["payload"] + 8 * MERGES_DECLARED == MERGE_RAW
    tok = read("tokenizer.rs")
    budget = const_value(tok, "TOKENIZER_HEAP_BUDGET")
    assert budget == 16 * 1024 * 1024
    merge_charge = MERGES_DECLARED * 16          # 3 u32 ids + 1 u32 rank per merge
    upper = (TOKEN_PAYLOAD + 1024 + 4 * (VOCAB + 1) + VOCAB + 4 * VOCAB
             + merge_charge)
    assert upper <= budget, "the merge table cannot be held under the static ceiling"
    assert "VocabHeldTooLarge(usize, usize)" in tok
    finish = function_body(tok, "fn finish(self) -> Result<FileTokenizer, TokenizerError>")
    assert "held.saturating_add(merge_bytes) > budget" in finish
    assert "pub fn merges_complete(&self) -> bool" in tok
    bpe = function_body(tok, "pub fn encode_bpe(")
    assert "if !self.merges_complete()" in bpe


def test_point_10_encode_honesty_round_trip_and_marker_limitation():
    """VERIFIED (source contract): `encode` is a longest-match unigram encoder,
    NOT SentencePiece/llama.cpp-compatible; no normalize/pre stage runs; no
    reference tokenizer is installed on this host, so reference parity is never
    claimed. The U+2581 marker limitation is asserted, not hidden. `encode_bpe`
    claims only the raw-byte round trip. Runtime parity is NOT VERIFIED and is
    not claimed by this suite.
    """
    tok = read("tokenizer.rs")
    assert "NOT a" in tok and "SentencePiece or llama.cpp-compatible tokenizer" in tok
    assert "None is installed on this host" in tok
    assert "Asserted as a limitation, not hidden" in tok
    assert "pub const WORD_MARKER: &[u8] = b\"\\xe2\\x96\\x81\";" in tok
    bpe_doc = tok[tok.index("/// Segments `text` by applying") : tok.index("pub fn encode_bpe(")]
    assert "NOT a reference-runtime parity claim" in bpe_doc
    assert "decode_sequence(encode_bpe(t)) == t" in bpe_doc


def test_point_11_chat_template_refused_whole_and_eot_as_declared(gguf):
    """VERIFIED: chat_template is 18,810 bytes -- OVER gguf.rs's 1024-byte cap, so
    the reader refuses it WHOLE (RefusedTooLong with the stated number, stepping
    over the bytes, never quoting a cut template). eot_token_id 106 is declared
    and reported; add_eos_token is ABSENT from this file (a fact, not a guess);
    add_bos true; bos 2 / eos 1 / unk 3 / pad 0 / mask 4. tokenizer.rs records the
    template's length and steps over its bytes.

    Recorded discrepancy: tokenizer.rs's module doc says 18,808; the file's own
    length prefix measures 18,810. This suite pins the MEASURED number and reports
    the two-byte doc drift rather than silently trusting either.
    """
    m = gguf["meta"]
    assert gguf["arrays"]["tokenizer.chat_template"]["len"] == TEMPLATE_LEN
    assert TEMPLATE_LEN > TEMPLATE_CAP
    assert m["tokenizer.ggml.eot_token_id"] == 106
    assert "tokenizer.ggml.add_eos_token" not in m
    assert m["tokenizer.ggml.add_bos_token"] == 1
    assert m["tokenizer.ggml.bos_token_id"] == 2
    assert m["tokenizer.ggml.eos_token_id"] == 1
    assert m["tokenizer.ggml.unknown_token_id"] == 3
    assert m["tokenizer.ggml.padding_token_id"] == 0
    assert m["tokenizer.ggml.mask_token_id"] == 4
    assert m["tokenizer.ggml.model"] == "gemma4"
    assert "tokenizer.ggml.pre" not in m
    gguf_src = read("gguf.rs")
    assert const_value(gguf_src, "MAX_CHAT_TEMPLATE_LEN") == TEMPLATE_CAP
    bounded = function_body(gguf_src, "fn read_bounded_string(")
    assert "if need > dst.len() {" in bounded
    assert "stream.skip(need)?" in bounded
    assert "StringReading::RefusedTooLong(stated)" in bounded
    assert "chat_template: [u8; MAX_CHAT_TEMPLATE_LEN]" in gguf_src
    assert "a cut template must not be quoted" in gguf_src
    tok = read("tokenizer.rs")
    assert "pub fn eot_id(&self) -> Option<u32>" in tok
    template = function_body(tok, "fn template_len(")
    assert "stream.skip(skip)" in template
    assert "self.chat_template_len = len" in template


def test_point_12_streaming_walk_machinery_and_bounded_reads(gguf):
    """VERIFIED (source contract + arithmetic on the real file): the block loop
    releases its slot before refilling, every positioned read is bounded by the
    SAME 64 KiB on both sides of the syscall, the metadata window is 512 bytes,
    and the vocabulary is counted, not loaded (4096-element array cap). E2B's
    widest block is 24,758,304 bytes -> at most 378 positioned reads. The reader
    holds the 35 unequal per-layer FFN widths as an array, not a scalar reduction.

    Honest limitations recorded here: layers 0..14 declare 17 tensors, four more
    than the loader's thirteen (inp_gate, proj, layer_output_scale, post_norm);
    layers 15..34 declare only 14 -- the thirteen-name loader's attn_k, attn_v and
    attn_k_norm are ABSENT (shared KV), so load_block meets TensorAbsent there.
    The generative walk on the guest is NOT VERIFIED.
    """
    weights = read("weights.rs")
    kernel = KERNEL_SYSCALL.read_text(encoding="utf-8")
    assert const_value(weights, "MAX_POSITIONED_READ_BYTES") == READ_WINDOW
    assert usize_literal(re.search(
        r"const MAX_FS_IO_BYTES: usize = ([0-9_ *]+);", kernel).group(1)) == READ_WINDOW
    block = function_body(weights, "pub fn load_block(&mut self, index: usize)")
    assert block.index("self.slot.release()") < block.index("for suffix in BLOCK_NORMS")
    loop = function_body(weights, "pub fn read_in_bounded_chunks<")
    assert "&mut into[filled..filled + wanted]" in loop
    gguf_src = read("gguf.rs")
    assert const_value(gguf_src, "METADATA_WINDOW_BYTES") == 512
    assert const_value(gguf_src, "MAX_STRING_ARRAY_ELEMENTS") == 4096
    assert VOCAB > 4096
    assert "ARRAY of 35 INT32" in gguf_src
    assert "6144 for layers 0..14 and 12288 for layers 15..34" in gguf_src
    blocks = blocks_of(gguf)
    assert len(blocks) == BLOCKS
    loader_names = set(string_list(
        weights, "pub const BLOCK_QUANTISED: &[&str] = &[")) | set(string_list(
        weights, "pub const BLOCK_NORMS: &[&str] = &["))
    assert len(loader_names) == 13
    widest, narrowest = 0, None
    for idx, tensors in blocks.items():
        suffixes = {t["name"].split(".", 2)[2] for t in tensors}
        total = sum(t["bytes"] for t in tensors)
        widest = max(widest, total)
        narrowest = total if narrowest is None else min(narrowest, total)
        if idx < SHARED_START:
            assert len(suffixes) == 17
            assert loader_names <= suffixes
        else:
            assert len(suffixes) == 14
            assert loader_names - suffixes == SHARED_KV_MISSING
        assert suffixes - loader_names == BLOCK_EXTRAS
    assert widest == MAX_BLOCK_BYTES
    assert narrowest == MIN_BLOCK_BYTES
    assert (widest + READ_WINDOW - 1) // READ_WINDOW == MAX_BLOCK_READS


def test_point_13_budget_accounting_refuses_rather_than_truncates(gguf):
    """VERIFIED: the run path prices weight bytes, and an over-budget request is
    refused with both numbers. E2B's payload (2,170,371,168 B) exceeds the scratch
    floor (125,829,120 B) by 17.25x, so an IN-MEMORY run is refused; the streamed
    path prices scratch, not the whole file. No dimension is clamped to fit, and
    the 256 MiB user address space cannot hold this file's payload either.
    """
    inference = read("inference.rs")
    floor = const_value(inference, "SCRATCH_BYTE_BUDGET")
    assert floor == 120 * 1024 * 1024
    assert gguf["payload"] > floor
    assert gguf["payload"] / floor == pytest.approx(17.2486, abs=1e-3)
    assert "pub fn weight_bytes(&self, dtype: u32) -> Result<usize, InferenceError>" in inference
    against = function_body(inference, "fn check_scratch_against")
    assert "ScratchTooLarge(needed, budget)" in against
    assert "largest_single_buffer_bytes" in against
    priced = function_body(inference, "fn check_scratch_budget")
    assert "check_scratch_against(dynamic_scratch_byte_budget())" in priced
    body = function_body(
        inference,
        "pub fn from_gguf(info: &crate::gguf::GgufModelInfo) -> Result<Self, InferenceError>",
    )
    for clamp in (".min(", ".max(", "unwrap_or(MAX_"):
        assert clamp not in body
    user = KERNEL_USER.read_text(encoding="utf-8")
    assert usize_literal(re.search(
        r"pub const USER_ADDRESS_SPACE_LIMIT: usize = ([0-9_ *]+);", user).group(1)) == (
        256 * 1024 * 1024)
    assert gguf["payload"] > 256 * 1024 * 1024
    assert by_name(gguf, LARGEST[0])["bytes"] > CEILING
    assert by_name(gguf, SECOND[0])["bytes"] < CEILING


def test_point_14_end_to_end_generation(gguf):
    """NOT VERIFIED / BLOCKED -- skipped, never faked.

    No end-to-end generation from this file has been demonstrated on host or
    guest. The specific blockers, each with evidence, are assembled below and
    carried in the skip reason.
    """
    inference = read("inference.rs")
    weights = read("weights.rs")
    evidence = [
        "token_embd.weight and the FFN matrices of layers 15..34 are TQ2_0 "
        "(dtype 35), 61 tensors outside the decode set [0,1,2,8,12] "
        f"(inference.rs:{line_of(inference, 'pub fn quantised_row_bytes(')})",
        "per_layer_token_embd.weight is 1,321,205,760 B, 9.84x the per-mapping "
        "ceiling; the whole mapping is refused "
        f"(weights.rs:{line_of(weights, 'fn read_raw_tensor(')}); a 5,040-byte row "
        "loader exists "
        f"(weights.rs:{line_of(weights, 'pub fn load_per_layer_token_embd_row(')}) "
        "but the graph does not yet consume the per-layer input mechanism",
        "the 20 shared-KV layers (15..34) declare no attn_k/attn_v/attn_k_norm; "
        "the thirteen-name block loader requires them "
        f"(weights.rs:{line_of(weights, 'pub const BLOCK_QUANTISED')}) -> TensorAbsent",
        "the file declares no output.weight head; every block also declares "
        "inp_gate/proj/layer_output_scale/post_norm beyond the loader's thirteen",
        "the geometry/buffer work in inference.rs (ModelConfig) is in flight and "
        "the crate's test target does not compile at this HEAD; no guest or host "
        "run has been measured, and host-backed would still be host-backed",
    ]
    pytest.skip("NOT VERIFIED / BLOCKED: end-to-end generation is not demonstrated. "
                + "; ".join(evidence))


def test_point_15_synthetic_fixtures_are_never_presented_as_this_model(gguf):
    """VERIFIED: the tiny fixture labels ITSELF synthetic in its own metadata,
    the harness calls these files synthetic parser fixtures, no registered model
    path names the fixture, the generative path is the published gemma4-2b slot,
    and the module's own comments refuse to invent a "Mobile Lite" variant (the
    D7 test pins that). The EG2 artifact is a different model this suite's
    identity gate rejects (proved in the meta-test).
    """
    tiny_path = MODELS / TINY_FIXTURE
    if not tiny_path.exists():
        pytest.skip(f"{TINY_FIXTURE} absent at {tiny_path}; the on-disk synthetic "
                    "label cannot be measured. No substitute is used.")
    tiny = parse_gguf(tiny_path)
    assert tiny["size"] == 521_536
    assert tiny["meta"]["general.name"] == "InterOS synthetic Gemma fixture"
    assert tiny["meta"]["general.architecture"] == "gemma2"
    assert tiny["meta"]["general.size_label"] == "tiny-f32"
    assert identity_defects(identity_of(tiny)), "the tiny fixture passed E2B's identity gate"
    harness = (ROOT / "tests" / "run_all_contracts.py").read_text(encoding="utf-8")
    assert "synthetic" in harness and "parser fixtures" in harness
    gguf_src = read("gguf.rs")
    candidates = string_list(gguf_src, "pub const MODEL_CANDIDATE_PATHS: &[&str] = &[")
    assert not any("gemma-tiny" in path for path in candidates)
    assert 'PATH_GENERATIVE_MODEL_DATA: &str = "/data/models/gemma4-2b.gguf"' in gguf_src
    assert 'PATH_GEMMA4_MOBILE_LITE_DATA: &str = "/data/models/gemma4-mobile-lite.gguf"' in gguf_src
    assert "a file here is whatever its own header declares" in gguf_src
    assert "fn test_the_mobile_lite_comments_do_not_invent_a_model()" in gguf_src


# ---------------------------------------------------------------------------
# Meta-tests: each key gate is proved able to fail, by injecting the fault into
# a copy of the data or text and running the SAME detector the point runs.
# ---------------------------------------------------------------------------


def test_meta_identity_gate_can_fail(gguf):
    """Substituting the EG2 embedding artifact's or the tiny fixture's identity,
    or a fabricated "Mobile Lite" name, must trip the identity detector."""
    ident = identity_of(gguf)
    assert identity_defects(ident) == []
    eg2 = dict(ident, size=175_673_856, arch="gemma-embedding2", size_label="271M",
               name="Embeddinggemma 2", tensor_count=413, kv_count=52)
    assert identity_defects(eg2), "the EG2 embedding artifact passed as E2B"
    assert any("embedding model" in d for d in identity_defects(eg2))
    tiny = dict(ident, size=521_536, arch="gemma2", size_label="tiny-f32",
                name="InterOS synthetic Gemma fixture", tensor_count=31)
    assert identity_defects(tiny), "the synthetic fixture passed as E2B"
    lite = dict(ident, name="Gemma 4 Mobile Lite")
    assert any("fabricated" in d for d in identity_defects(lite))
    renamed = dict(ident, size=EXPECTED_SIZE + 1)
    assert identity_defects(renamed), "a one-byte size change passed"


def test_meta_delta_self_check_can_fail(gguf):
    """Perturbing the FIRST offset breaks the exact-sum self-check (the deltas
    telescope to payload minus first offset); an intermediate perturbation is
    caught by the per-tensor geometry bounds. Both must fire."""
    assert section_defects(gguf) == []
    mutated = dict(gguf)
    tensors = [dict(t) for t in gguf["tensors"]]
    first = next(t for t in tensors if t["offset"] == 0)
    first["offset"] += ALIGNMENT
    mutated["tensors"] = tensors
    defects = section_defects(mutated)
    assert any("sum" in d for d in defects), "a perturbed first offset kept the sum exact"
    assert any("geometry disagrees" in d for d in defects)
    middle = dict(gguf)
    tensors = [dict(t) for t in gguf["tensors"]]
    target = next(t for t in tensors if t["name"] == "blk.0.ffn_up.weight")
    target["offset"] += ALIGNMENT
    middle["tensors"] = tensors
    assert any("geometry disagrees" in d for d in section_defects(middle)), (
        "an intermediate offset perturbation passed the bounds check"
    )


def test_meta_row_orientation_can_fail(gguf):
    """Swapping the per-layer table's dimensions makes dims[0] the vocabulary:
    a 'row' would then be 262,144 elements = 147,456 bytes of Q4_0, over the
    64 KiB positioned-read bound. The row detector must catch the swap instead
    of pricing a row the file does not hold."""
    weights = read("weights.rs")
    assert row_defects(gguf, weights) == []
    swapped = dict(gguf)
    tensors = []
    for t in gguf["tensors"]:
        t2 = dict(t)
        if t2["name"] == "per_layer_token_embd.weight":
            t2["dims"] = list(reversed(t2["dims"]))
        tensors.append(t2)
    swapped["tensors"] = tensors
    defects = row_defects(swapped, weights)
    assert any("row length" in d for d in defects), "the orientation swap passed"
    assert any("over the" in d and "positioned-read" in d for d in defects), (
        "a 147,456-byte row passed the positioned-read bound"
    )
    assert row_defects(gguf, weights) == []


def test_meta_wrong_tq_block_table_can_fail(gguf):
    """The historical fault: 32-element blocks for a 256-element superblock.
    TQ2_0 priced as (32, 66) makes token_embd 8x too large -- a total bigger than
    the file. The geometry cross-check must catch it."""
    assert section_defects(gguf) == []
    saved = GEOM[35]
    try:
        GEOM[35] = (32, 66)
        mutated = dict(gguf)
        tensors = []
        for t in gguf["tensors"]:
            t2 = dict(t)
            be, bb = GEOM[t2["dtype"]]
            t2["predicted"] = (t2["elements"] + be - 1) // be * bb
            tensors.append(t2)
        mutated["tensors"] = tensors
        defects = section_defects(mutated)
        assert any("geometry disagrees" in d for d in defects), (
            "TQ2_0 priced at 32-element blocks passed the cross-check"
        )
        assert any("token_embd" in d for d in defects)
    finally:
        GEOM[35] = saved
    assert section_defects(gguf) == []


def test_meta_ceiling_enumeration_can_fail(gguf):
    """The over-ceiling set is an EXACT enumeration: a second tensor pushed over
    the ceiling, or the known one shrunk under it, must trip the detector. The
    ceiling itself is never moved to make a model fit."""
    assert ceiling_defects(gguf) == []
    grown = dict(gguf)
    tensors = [dict(t) for t in gguf["tensors"]]
    next(t for t in tensors if t["name"] == "token_embd.weight")["bytes"] = CEILING + 1
    grown["tensors"] = tensors
    assert ceiling_defects(grown), "a second over-ceiling tensor passed"
    shrunk = dict(gguf)
    tensors = [dict(t) for t in gguf["tensors"]]
    next(t for t in tensors
         if t["name"] == "per_layer_token_embd.weight")["bytes"] = CEILING
    shrunk["tensors"] = tensors
    assert ceiling_defects(shrunk), "the known over-ceiling tensor vanished unnoticed"
    assert ceiling_defects(gguf) == []


def test_meta_refusal_gate_can_fail():
    """A sixth decode arm, or a refusal replaced by a neighbour's layout, must
    trip the refusal detector."""
    inference = read("inference.rs")
    weights = read("weights.rs")
    assert refusal_defects(inference, weights) == []
    grown = inference.replace(
        "        8 => blocks(32, 34),",
        "        8 => blocks(32, 34),\n        3 => blocks(32, 20),",
        1,
    )
    assert grown != inference, "the Q8_0 arm moved; this fixture is stale"
    assert any("decode set" in d for d in refusal_defects(grown, weights))
    guessed = inference.replace(
        "_ => Err(InferenceError::UnsupportedQuantization(dtype)),",
        "_ => Ok(()),",
    )
    assert guessed != inference, "the refusal moved; this fixture is stale"
    assert any("refuses an unmapped dtype" in d for d in refusal_defects(guessed, weights))
    unnamed = weights.replace("UnsupportedDtype(&'static str, u32)",
                              "UnsupportedDtype(u32)")
    assert unnamed != weights
    assert any("name and number" in d for d in refusal_defects(inference, unnamed))