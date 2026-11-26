# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Per-model acceptance suite: EmbeddingGemma 2 (`embeddinggemma-2-UD-Q4_K_XL.gguf`).

WHY a separate suite per model: the mandate requires fifteen verification points
against EACH genuine target artifact, and a resource-blocked model may never be
silently replaced by a smaller fixture. This suite measures the real 175,673,856-byte
file on this host -- its header, metadata and descriptor table only, never a tensor
payload -- and pins the engine's source contracts beside those measurements. The
identity facts here correct earlier wrong claims: this file's `size_label` is "271M"
(740M is Google's total multimodal parameter count, not this text GGUF's identity),
its hidden width is 512 (768 is `embedding_length_out`), and the 300M predecessor
(`embeddinggemma-300M-Q8_0.gguf`, arch `gemma-embedding`) is a DIFFERENT,
previous-generation model that must never be presented as this one.

HONESTY RULE: point 14 (end-to-end embedding generation) is NOT VERIFIED. Two of
this file's tensors use quantisations the engine's decode set [0, 1, 2, 8, 12] does
not map (Q5_K 13 and Q6_K 14, one tensor each), every block declares four per-layer
input tensors the loader's thirteen-tensor block does not yet consume, and the
geometry/buffer work in `inference.rs` is in flight. A suite reporting 15/15 green
without a demonstrated run would be a fabricated success report. Blocked points
skip with the specific reason and file:line evidence; no point is deleted or
weakened to get green, and no fixture's output is asserted as this model's.

Tensor sizes are measured by offset deltas (sort by offset; size = next offset minus
this offset; last = payload end minus offset). The deltas sum EXACTLY to the payload
section -- a self-check no dtype table can fake. The earlier probe
`target/verify/tensor_ceiling.py` is WRONG (32-element blocks for K-quants) and is
never used or cited here; `target/verify/tensor_offsets.py` is the lead's method.
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

ARTIFACT = "embeddinggemma-2-UD-Q4_K_XL.gguf"
PREDECESSOR = "embeddinggemma-300M-Q8_0.gguf"
TINY_FIXTURE = "gemma-tiny-f32.gguf"

# Identity, lead-measured and re-measured here from the file's own bytes.
EXPECTED_SIZE = 175_673_856
EXPECTED_TENSORS = 413
EXPECTED_KV = 52
EXPECTED_ARCH = "gemma-embedding2"
EXPECTED_SIZE_LABEL = "271M"          # NOT 740M: that is the multimodal total.
EXPECTED_NAME = "Embeddinggemma 2"

# Section geometry. `KV_END` is the lead's "metadata_end"; the lead's "data section"
# (159,907,491) is file size minus KV_END and therefore INCLUDES the 24,475-byte
# descriptor table plus 8 bytes of alignment padding. The exact payload section
# starts at DATA_START, after the descriptor table, and is 159,883,008 bytes.
KV_END = 15_766_365
TABLE_END = 15_790_840
ALIGNMENT = 32
DATA_START = 15_790_848
PAYLOAD = 159_883_008
LEAD_SECTION = 159_907_491

CEILING = 134_213_632                 # per-mapping ceiling, never weakened
READ_WINDOW = 65_536                  # positioned-read bound, both sides
MAPPED_DTYPES = [0, 1, 2, 8, 12, 13, 14, 35]  # the engine's DECODE set (inference.rs)

# GGML block geometry for PLACEMENT arithmetic: (elements per block, bytes per
# block). Legacy quants are 32-element blocks; K-quants and TQ are 256-element
# superblocks. Q4_0 and Q4_K both give 0.5625 B/element, which is why a wrong table
# can look right -- the delta self-check, not this table, is authoritative.
GEOM = {
    0: (1, 4), 1: (1, 2), 2: (32, 18), 3: (32, 20), 6: (32, 22),
    7: (32, 24), 8: (32, 34), 9: (32, 36), 10: (256, 84), 11: (256, 110),
    12: (256, 144), 13: (256, 176), 14: (256, 210), 15: (256, 292),
    30: (1, 2), 35: (256, 66),
}

EXPECTED_CENSUS = {0: 194, 8: 48, 12: 169, 13: 1, 14: 1}
EXPECTED_UNMAPPED = {}
EXPECTED_OVER = {}                    # every EG2 tensor fits the mapping ceiling
LARGEST = ("token_embd.weight", 75_497_472, 12, [512, 262144])

VOCAB = 262_144
TOKEN_PAYLOAD = 2_316_295
MAX_PIECE = 93
MERGES_DECLARED = 514_906
MERGE_PAYLOAD = 5_132_471
MERGE_RAW = 9_251_719                 # payload + 8-byte length prefix per entry
TEMPLATE_LEN = 1_016
BLOCKS = 24
BLOCK_TENSOR_COUNT = 17
BLOCK_EXTRAS = {"inp_gate.weight", "proj.weight", "layer_output_scale.weight",
                "post_norm.weight"}
MAX_BLOCK_BYTES = 3_815_456

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
    if ident.get("size_label") == "740M":
        defects.append("size_label claims the multimodal total, not this text GGUF")
    if ident.get("arch") == "gemma-embedding":
        defects.append("this is the 300M predecessor's architecture, not EG2's")
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
        defects.append(f"undecodable tensors {unmapped} != {EXPECTED_UNMAPPED}")
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


@pytest.fixture(scope="module")
def gguf():
    path = MODELS / ARTIFACT
    if not path.exists():
        pytest.skip(
            f"{ARTIFACT} is absent at {path}. This suite measures the genuine "
            "EmbeddingGemma 2 artifact and never substitutes a smaller fixture."
        )
    return parse_gguf(path)


# ---------------------------------------------------------------------------
# The fifteen verification points.
# ---------------------------------------------------------------------------


def test_point_01_artifact_identity(gguf):
    """VERIFIED: exact byte size, GGUF magic/version, declared arch and size_label.

    A renamed or substituted file is caught here: the 300M predecessor and the
    synthetic tiny fixture both fail this detector (proved in the meta-test).
    """
    assert identity_defects(identity_of(gguf)) == []
    assert gguf["meta"]["general.basename"] == "embeddinggemma"
    assert gguf["meta"]["general.file_type"] == 15


def test_point_02_tensor_count_sections_and_delta_self_check(gguf):
    """VERIFIED: 413 tensors; the offset deltas sum EXACTLY to the payload section.

    Also pins the lead's figures: metadata_end 15,766,365 and 'data section'
    159,907,491 (which includes the descriptor table). Every tensor's delta sits
    between its geometry-predicted bytes and that prediction aligned up to 32 --
    the cross-check that catches a wrong block table.
    """
    assert gguf["tensor_count"] == EXPECTED_TENSORS
    assert section_defects(gguf) == []


def test_point_03_geometry_is_read_from_this_files_own_metadata(gguf):
    """VERIFIED: hidden 512, out 768, 24 layers, 4 heads, per-layer KV and SWA.

    Nothing is hardcoded from a different model: every width is cross-checked
    against the tensor descriptors it sizes, per layer, including the SWA layers'
    narrower 256-wide heads and the full-attention layers' 512-wide heads.
    """
    m = gguf["meta"]
    arch = EXPECTED_ARCH
    assert m[f"{arch}.embedding_length"] == 512
    assert m[f"{arch}.embedding_length_out"] == 768
    assert m[f"{arch}.block_count"] == BLOCKS
    assert m[f"{arch}.attention.head_count"] == 4
    assert m[f"{arch}.attention.key_length"] == 512
    assert m[f"{arch}.attention.key_length_swa"] == 256
    assert m[f"{arch}.attention.value_length"] == 512
    assert m[f"{arch}.attention.value_length_swa"] == 256
    assert m[f"{arch}.context_length"] == 262_144
    assert m[f"{arch}.feed_forward_length"] == 2048
    assert m[f"{arch}.embedding_length_per_layer_input"] == 512
    assert m[f"{arch}.attention.sliding_window"] == 1024
    assert m[f"{arch}.pooling_type"] == 1
    kv = m[f"{arch}.attention.head_count_kv"]["values"]
    swa = m[f"{arch}.attention.sliding_window_pattern"]["values"]
    assert len(kv) == BLOCKS and len(swa) == BLOCKS
    heads = 4
    blocks = blocks_of(gguf)
    assert sorted(blocks) == list(range(BLOCKS))
    for idx, tensors in blocks.items():
        named = {t["name"].split(".", 2)[2]: t for t in tensors}
        key_len = 256 if swa[idx] else 512
        val_len = 256 if swa[idx] else 512
        assert named["attn_q.weight"]["dims"] == [512, heads * key_len]
        assert named["attn_k.weight"]["dims"] == [512, kv[idx] * key_len]
        assert named["attn_v.weight"]["dims"] == [512, kv[idx] * val_len]
        assert named["attn_output.weight"]["dims"] == [heads * val_len, 512]
        assert named["attn_q_norm.weight"]["dims"] == [key_len]
        assert named["attn_k_norm.weight"]["dims"] == [key_len]
        assert named["ffn_gate.weight"]["dims"] == [512, 2048]
        assert named["ffn_down.weight"]["dims"] == [2048, 512]
    assert by_name(gguf, "token_embd.weight")["dims"] == [512, VOCAB]
    assert by_name(gguf, "output.weight")["dims"] == [512, 768]
    # heads_span the engine prices: 4 x 512 = 2048 over the declared key_length.
    assert heads * m[f"{arch}.attention.key_length"] == 2048


def test_point_04_every_tensor_fits_the_per_mapping_ceiling(gguf):
    """VERIFIED: zero tensors over 134,213,632 B; largest is token_embd at 0.56x.

    The ceiling is read from the engine's own constant, not remembered, and is
    never weakened to make a model fit.
    """
    assert const_value(read("inference.rs"), "LARGEST_SINGLE_BUFFER_BYTES") == CEILING
    assert ceiling_defects(gguf) == []
    largest = by_name(gguf, LARGEST[0])
    assert largest["bytes"] / CEILING == pytest.approx(0.5625, abs=1e-4)


def test_point_05_placement_geometry_is_not_the_decode_set(gguf):
    """VERIFIED: the file is fully PLACEABLE; two tensors remain UNDECODABLE.

    weights.rs's `ggml_block_geometry` sizes sixteen dtype ids for placement
    arithmetic (including Q5_K 13 and Q6_K 14), while row decoding goes through
    the engine's five-id `quantised_row_bytes`. Every EG2 tensor's delta matches
    the placement geometry -- so the walk can place all 413 tensors, and the two
    unmapped ones are refused by name and number at decode, not mis-sized.
    """
    weights = read("weights.rs")
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


def test_point_06_dtype_census_and_the_undecodable_tensors(gguf):
    """VERIFIED: census {0:194, 8:48, 12:169, 13:1, 14:1}; the engine cannot
    decode `output.weight` (Q6_K, 14) or `per_layer_model_proj.weight` (Q5_K, 13).

    This is a real, honest limitation: the decode set is pinned at five ids by
    tests/test_inference_engine_graph_contract.py and is not widened here.
    """
    inference = read("inference.rs")
    assert mapped_dtype_ids(inference) == MAPPED_DTYPES
    assert census_defects(gguf, MAPPED_DTYPES) == []
    for name, dtype in EXPECTED_UNMAPPED.items():
        t = by_name(gguf, name)
        assert t["dtype"] == dtype and t["dtype"] not in MAPPED_DTYPES


def test_point_07_the_refusal_names_the_tensor_and_the_number():
    """VERIFIED (source contract): an unmapped dtype is refused, never read as a
    neighbour's layout. Runtime execution of the refusal is NOT VERIFIED while
    the crate's test target does not compile (inference.rs geometry work in
    flight); the mutation meta-test below proves this gate can fail.
    """
    assert refusal_defects(read("inference.rs"), read("weights.rs")) == []


def test_point_08_vocabulary_comes_from_the_file_with_no_fallback(gguf):
    """VERIFIED: 262,144 entries counted from the array's own elements, byte
    tokens <0x00>..<0xFF> at ids 238..=493, and the reader refuses a file with no
    vocabulary rather than guessing one. The invented COMMON_SUBWORDS table is
    gone from production code.
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
    # Upper bound on the held vocabulary: piece bytes + byte map + starts +
    # matchable flags + a pool no larger than one entry per id, plus the merges.
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


def test_point_11_chat_template_and_eot_as_this_file_declares_them(gguf):
    """VERIFIED: chat_template 1,016 bytes (fits gguf.rs's 1024-byte window),
    eot_token_id ABSENT (reported as None, never invented), add_eos_token true,
    bos 2 / eos 1 / unk 3 / pad 0. tokenizer.rs records the template's length and
    steps over its bytes; gguf.rs refuses an over-long template whole.
    """
    m = gguf["meta"]
    assert gguf["arrays"]["tokenizer.chat_template"]["len"] == TEMPLATE_LEN
    assert "tokenizer.ggml.eot_token_id" not in m
    assert m["tokenizer.ggml.add_eos_token"] == 1
    assert m["tokenizer.ggml.bos_token_id"] == 2
    assert m["tokenizer.ggml.eos_token_id"] == 1
    assert m["tokenizer.ggml.unknown_token_id"] == 3
    assert m["tokenizer.ggml.padding_token_id"] == 0
    assert m["tokenizer.ggml.model"] == "gemma4"
    assert "tokenizer.ggml.pre" not in m
    gguf_src = read("gguf.rs")
    assert const_value(gguf_src, "MAX_CHAT_TEMPLATE_LEN") == 1024
    assert TEMPLATE_LEN <= 1024
    tok = read("tokenizer.rs")
    assert "pub fn eot_id(&self) -> Option<u32>" in tok
    template = function_body(tok, "fn template_len(")
    assert "stream.skip(skip)" in template
    assert "self.chat_template_len = len" in template


def test_point_12_streaming_walk_order_and_bounded_reads(gguf):
    """VERIFIED (source contract + arithmetic on the real file): the streamed run
    walks embedding row -> blocks -> pool -> head -> L2 in order, one block slot
    at a time, and every positioned read is bounded by the SAME 64 KiB on both
    sides of the syscall. EG2's largest block is 3,815,456 bytes -> at most 59
    positioned reads; the metadata window is 512 bytes and the vocabulary is
    counted, not loaded. Runtime walk on the guest is NOT VERIFIED.

    Honest limitation recorded here: each EG2 block declares 17 tensors, four
    more than the loader's thirteen (inp_gate, proj, layer_output_scale,
    post_norm) -- the per-layer input mechanism the graph does not yet consume.
    """
    weights = read("weights.rs")
    kernel = KERNEL_SYSCALL.read_text(encoding="utf-8")
    assert const_value(weights, "MAX_POSITIONED_READ_BYTES") == READ_WINDOW
    assert usize_literal(re.search(
        r"const MAX_FS_IO_BYTES: usize = ([0-9_ *]+);", kernel).group(1)) == READ_WINDOW
    body = function_body(weights, "pub fn run_streamed_embedding<S: PositionRead>(")
    order = ["load_embedding_row", "embedding_row_into", "load_block", "run_block",
             "rms_norm(", "mean_of(", "run_pooling_head", "l2_normalise("]
    positions = [body.index(step) for step in order]
    assert positions == sorted(positions)
    block = function_body(weights, "pub fn load_block(&mut self, index: usize)")
    assert block.index("self.slot.release()") < block.index("for suffix in BLOCK_NORMS")
    loop = function_body(weights, "pub fn read_in_bounded_chunks<")
    assert "&mut into[filled..filled + wanted]" in loop
    gguf_src = read("gguf.rs")
    assert const_value(gguf_src, "METADATA_WINDOW_BYTES") == 512
    assert const_value(gguf_src, "MAX_STRING_ARRAY_ELEMENTS") == 4096
    blocks = blocks_of(gguf)
    assert len(blocks) == BLOCKS
    loader_names = set(string_list(
        weights, "pub const BLOCK_QUANTISED: &[&str] = &[")) | set(string_list(
        weights, "pub const BLOCK_NORMS: &[&str] = &["))
    assert len(loader_names) == 13
    widest = 0
    for tensors in blocks.values():
        suffixes = {t["name"].split(".", 2)[2] for t in tensors}
        assert len(suffixes) == BLOCK_TENSOR_COUNT
        assert loader_names <= suffixes
        assert suffixes - loader_names == BLOCK_EXTRAS
        widest = max(widest, sum(t["bytes"] for t in tensors))
    assert widest == MAX_BLOCK_BYTES
    assert (widest + READ_WINDOW - 1) // READ_WINDOW == 59


def test_point_13_budget_accounting_refuses_rather_than_truncates(gguf):
    """VERIFIED: the run path prices weight bytes, and an over-budget request is
    refused with both numbers. EG2's payload (159,883,008 B) exceeds the scratch
    floor (125,829,120 B) by 1.27x, so an IN-MEMORY run is refused; the streamed
    path prices scratch, not the whole file. No dimension is clamped to fit.
    """
    inference = read("inference.rs")
    floor = const_value(inference, "SCRATCH_BYTE_BUDGET")
    assert floor == 120 * 1024 * 1024
    assert gguf["payload"] > floor
    assert gguf["payload"] / floor == pytest.approx(1.2707, abs=1e-3)
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
    assert by_name(gguf, LARGEST[0])["bytes"] < CEILING


def test_point_14_end_to_end_embedding_generation(gguf):
    """NOT VERIFIED / BLOCKED -- skipped, never faked.

    No end-to-end embedding from this file has been demonstrated on host or
    guest. The specific blockers, each with evidence, are assembled below and
    carried in the skip reason.
    """
    inference = read("inference.rs")
    weights = read("weights.rs")
    evidence = [
        "output.weight is Q6_K (dtype 14) and per_layer_model_proj.weight is Q5_K "
        "(dtype 13); the decode set is [0,1,2,8,12] "
        f"(inference.rs:{line_of(inference, 'pub fn quantised_row_bytes(')})",
        "every block declares inp_gate/proj/layer_output_scale/post_norm beyond the "
        "loader's thirteen-tensor block (weights.rs BLOCK_QUANTISED/BLOCK_NORMS at "
        f"weights.rs:{line_of(weights, 'pub const BLOCK_QUANTISED')})",
        "SWA layers declare 256-wide heads while full-attention layers declare 512; "
        "the geometry/buffer work in inference.rs (ModelConfig) is in flight and the "
        "crate's test target does not compile at this HEAD",
        "no guest run has been measured; host-backed would still be host-backed",
    ]
    pytest.skip("NOT VERIFIED / BLOCKED: end-to-end embedding generation is not "
                "demonstrated. " + "; ".join(evidence))


def test_point_15_synthetic_fixtures_are_never_presented_as_this_model(gguf):
    """VERIFIED: the tiny fixture labels ITSELF synthetic in its own metadata,
    the harness calls these files synthetic parser fixtures, no registered model
    path names the fixture, and the 300M predecessor is a different architecture
    this suite's identity gate rejects (proved in the meta-test).
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
    assert identity_defects(identity_of(tiny)), "the tiny fixture passed EG2's identity gate"
    harness = (ROOT / "tests" / "run_all_contracts.py").read_text(encoding="utf-8")
    assert "synthetic" in harness and "parser fixtures" in harness
    gguf_src = read("gguf.rs")
    candidates = string_list(gguf_src, "pub const MODEL_CANDIDATE_PATHS: &[&str] = &[")
    assert not any("gemma-tiny" in path for path in candidates)
    assert 'PATH_EMBEDDING_MODEL_DATA: &str = "/data/models/embeddinggemma-2.gguf"' in gguf_src
    pred_path = MODELS / PREDECESSOR
    if pred_path.exists():
        pred = parse_gguf(pred_path)
        assert pred["size"] == 333_590_944
        assert pred["meta"]["general.architecture"] == "gemma-embedding"
        assert pred["meta"]["general.size_label"] == "300M"
        assert pred["meta"]["gemma-embedding.embedding_length"] == 768
        assert identity_defects(identity_of(pred)), "the predecessor passed EG2's gate"


# ---------------------------------------------------------------------------
# Meta-tests: each key gate is proved able to fail, by injecting the fault into
# a copy of the data or text and running the SAME detector the point runs.
# ---------------------------------------------------------------------------


def test_meta_identity_gate_can_fail(gguf):
    """Substituting the predecessor's or the tiny fixture's identity, or the old
    wrong 740M label, must trip the identity detector."""
    ident = identity_of(gguf)
    assert identity_defects(ident) == []
    predecessor = dict(ident, size=333_590_944, arch="gemma-embedding",
                       size_label="300M", name="Embeddinggemma 300m",
                       tensor_count=316)
    assert identity_defects(predecessor), "the 300M predecessor passed as EG2"
    tiny = dict(ident, size=521_536, arch="gemma2", size_label="tiny-f32",
                name="InterOS synthetic Gemma fixture", tensor_count=31)
    assert identity_defects(tiny), "the synthetic fixture passed as EG2"
    wrong_label = dict(ident, size_label="740M")
    assert any("multimodal" in d for d in identity_defects(wrong_label))
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


def test_meta_wrong_block_table_can_fail(gguf):
    """The historical fault: 32-element blocks for a K-quant. Q4_K priced as
    (32, 144) makes token_embd 8x too large -- a total bigger than the file. The
    geometry cross-check must catch it. (Q4_K as (32, 18) coincidentally matches
    0.5625 B/elem, which is exactly why the delta sum, not the table, rules.)"""
    assert section_defects(gguf) == []
    saved = GEOM[12]
    try:
        GEOM[12] = (32, 144)
        # Re-price the already-parsed tensors with the wrong table instead of
        # re-reading the file: the detector consumes `predicted`.
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
            "a K-quant priced at 32-element blocks passed the cross-check"
        )
        assert any("token_embd" in d for d in defects)
    finally:
        GEOM[12] = saved
    assert section_defects(gguf) == []


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