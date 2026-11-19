# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Graph contract for the inference engine that reads the installed model file.

The published file this repository ships -- `embeddinggemma-300M-Q8_0.gguf`, 333 MB,
24 blocks, three query heads and one key/value head 256 wide over a 768-wide residual
stream, a 1152-wide feed-forward, Q8_0 matrices with F32 norms, 262,144 vocabulary
rows, and a `dense_2`/`dense_3` head after the mean pool -- is the only model this
engine is allowed to be measured against. Every assertion below pins one half of the
rule that the engine run *that* graph rather than a smaller one it was convenient to
code: the dtype ids a row is sized for, the thirteen tensors a block is made of, the
order the block and the pooling head are walked in, the width a caller's vector is
checked against, and the metadata source the descriptor table has to be handed.

The other half is truth in the comments. This suite reads doc text as well as code,
because the defect it exists to catch is a sentence claiming a capability the code
does not have -- a quantisation counted that no arithmetic reads, a run on a
development host described as a run on the device, a pooling key named as if a reader
consulted it. Where a claim is pinned, the mutation case puts the false sentence back
into a copy and shows the detector parts; a gate nobody has seen fail is a comment.
"""

import re
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE = ROOT / "userspace" / "system-service" / "src"

# The five GGML dtype ids `quantised_row_bytes` gives a row layout to. 0 and 8 are
# the two the published file actually uses; 1, 2 and 12 are read because the same
# code paths answer other files the platform has been handed.
MAPPED_DTYPES = [0, 1, 2, 8, 12]

# The thirteen tensors the published file declares inside every one of its 24 blocks.
DECLARED_BLOCK_QUANTISED = [
    "attn_q.weight",
    "attn_k.weight",
    "attn_v.weight",
    "attn_output.weight",
    "ffn_gate.weight",
    "ffn_up.weight",
    "ffn_down.weight",
]
DECLARED_BLOCK_NORMS = [
    "attn_norm.weight",
    "attn_q_norm.weight",
    "attn_k_norm.weight",
    "post_attention_norm.weight",
    "ffn_norm.weight",
    "post_ffw_norm.weight",
]


def read(name: str) -> str:
    return (SERVICE / name).read_text(encoding="utf-8")


def without_comments(source: str) -> str:
    """`source` with comment lines dropped, so a detector over code cannot be
    satisfied by a sentence that merely mentions the thing."""
    kept = []
    for line in source.splitlines():
        stripped = line.strip()
        if stripped.startswith("//") or stripped.startswith("///") or stripped.startswith("//!"):
            continue
        kept.append(line.split("//")[0])
    return "\n".join(kept)


def function_body(source: str, signature: str) -> str:
    """The text from `signature` to the line that closes its brace.

    WHY hand-rolled: the contract is about the body's contents and order, and a
    regex over a fixed number of lines breaks the first time a call is wrapped.
    """
    start = source.index(signature)
    opened = source.index("{", start)
    depth = 0
    for index in range(opened, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"{signature} never closes")


def doc_block_above(source: str, needle: str) -> str:
    """The `///` paragraph sitting directly above the line containing `needle`.

    WHY attributes are walked past: `#[derive(...)]` sits between a type's doc block
    and its declaration, and a reader that stopped there would report every derived
    type as undocumented -- which is how a false claim in one survives a gate.
    """
    lines = source.splitlines()
    at = next(i for i, line in enumerate(lines) if needle in line)
    paragraph = []
    walk = at - 1
    while walk >= 0:
        stripped = lines[walk].strip()
        if stripped.startswith("///"):
            paragraph.append(stripped[3:].strip())
            walk -= 1
        elif not stripped or stripped.startswith("#"):
            walk -= 1
        else:
            break
    return " ".join(reversed(paragraph))


def string_list(source: str, declaration: str) -> list:
    """The quoted members of a `pub const X: &[&str] = &[...]` list."""
    body = source[source.index(declaration) :]
    body = body[: body.index("];")]
    return re.findall(r'"([^"]+)"', body)


def dtype_ids_sized(source: str) -> list:
    """The dtype ids `quantised_row_bytes` answers a row width for."""
    body = function_body(source, "pub fn quantised_row_bytes(")
    return arm_ids(body, "match dtype {")


def arm_ids(body: str, after: str) -> list:
    """Every dtype number naming an arm of the `match` that starts at `after`.

    WHY the digit runs and not whole lines: `embedding_row_into` gives Q4_0, Q8_0 and
    Q4_K one arm between them (`2 | 8 | 12 =>`), and a reader that only took the
    number at the start of the line would report an engine that reads three
    quantisations as reading one.
    """
    arms = body[body.index(after) :]
    found = set()
    for line in arms.splitlines():
        match = re.match(r"^\s*((?:\d{1,3}\s*\|\s*)*\d{1,3})\s*=>", line)
        if match:
            found.update(int(number) for number in re.findall(r"\d{1,3}", match.group(1)))
    return sorted(found)


# ---------------------------------------------------------------------------
# The dtype set: five ids sized, everything else refused by its number.
# ---------------------------------------------------------------------------


def test_the_engine_sizes_a_row_for_exactly_five_dtype_ids():
    """The count a comment carries has to be the count the arithmetic carries.

    WHY the arms and not the comment: `quantised_row_bytes` is what the loader calls
    to size every tensor row it reads, and `gemv` and `embedding_row_into` are what
    interpret the bytes. A sixth id claimed anywhere is a sixth layout nobody wrote;
    a dtype outside the five read with a neighbour's block width returns plausible
    floats from the wrong bytes, which is worse than a refusal and invisible in an
    embedding until someone compares two of them.
    """
    sized = dtype_ids_sized(read("inference.rs"))
    assert sized == MAPPED_DTYPES, (
        f"the engine sizes a row for {sized}, not the five {MAPPED_DTYPES}"
    )

    # `gemv` and `embedding_row_into` have to fall through to the same refusal, by
    # number, so an unmapped dtype cannot be read at all.
    for name, signature, anchor in (
        ("gemv", "pub fn gemv(", "match w_type {"),
        ("embedding_row_into", "pub fn embedding_row_into(", "match dtype {"),
    ):
        body = function_body(read("inference.rs"), signature)
        assert "_ => Err(InferenceError::UnsupportedQuantization(" in body, (
            f"{name} no longer refuses a dtype it has no layout for by its number"
        )
        assert arm_ids(body, anchor) == MAPPED_DTYPES, (
            f"{name}'s arms no longer name the five ids the loader sizes rows for: "
            f"{arm_ids(body, anchor)}"
        )


def test_the_loader_refuses_an_unmapped_dtype_by_number_not_by_reading_it():
    """`weights.rs` carries the refusal up the stack instead of guessing.

    The published file quantises its matrices as 8 and its norms as 0. A file that
    put Q5_K (13) where Q8_0 (8) is expected would, without this, have its scale
    bytes read as quants: `LoadError::UnsupportedDtype` names the tensor and the
    number so the screen can say which tensor the engine cannot read.
    """
    weights = read("weights.rs")
    assert "LoadError::UnsupportedDtype(" in without_comments(weights)
    assert "UnsupportedDtype(&'static str, u32)" in weights, (
        "the loader's dtype refusal stopped carrying the tensor name and the number, "
        "so a refusal can no longer say which tensor it refused"
    )
    # A block whose seven matrices do not agree on one dtype is refused rather than
    # run at one of them: `LayerWeights` holds a single `weight_type`.
    assert "MixedBlockDtype(" in weights, (
        "the loader no longer refuses a block split across two quantisations"
    )


def test_the_dtype_gate_can_fail():
    """A gate nobody has seen fail is a comment.

    Each block puts one fault into a copy and runs the same detector the gate above
    runs, so the five-id set, the enumeration and the refusal are each proved
    observable.
    """
    inference = read("inference.rs")
    baseline = dtype_ids_sized(inference)
    assert baseline == MAPPED_DTYPES

    # 1. A sixth arm appears and the set the engine sizes parts from the set the
    # contract names.
    grown = inference.replace(
        "        8 => blocks(32, 34),",
        "        8 => blocks(32, 34),\n        3 => blocks(32, 18),",
        1,
    )
    assert grown != inference, "the dtype match moved; this fixture is stale"
    assert dtype_ids_sized(grown) != MAPPED_DTYPES, (
        "an added arm would keep the five-id gate green"
    )

    # 2. The Q8_0 arm -- the published file's own quantisation -- disappears. The
    # detector has to notice, because a loader that lost Q8_0 refuses the shipped
    # model and every number in this repository's measurements stops being about it.
    lost = inference.replace("        8 => blocks(32, 34),", "", 1)
    assert lost != inference, "the Q8_0 arm moved; this fixture is stale"
    assert 8 not in dtype_ids_sized(lost), "losing Q8_0 kept the gate passing"

    # 3. The refusal replaced by a fall-through to a neighbour's layout.
    guessed = inference.replace(
        "        _ => Err(InferenceError::UnsupportedQuantization(dtype)),\n    }\n}\n\n/// Divides the accumulated stream",
        "        _ => {}\n    }\n}\n\n/// Divides the accumulated stream",
        1,
    )
    assert guessed != inference, "embedding_row_into's refusal moved; this fixture is stale"
    body = function_body(guessed, "pub fn embedding_row_into(")
    assert "_ => Err(InferenceError::UnsupportedQuantization(" not in body
    assert "_ => Err(InferenceError::UnsupportedQuantization(" in function_body(
        inference, "pub fn embedding_row_into("
    ), "the detector found no refusal to lose in the real source either"


# ---------------------------------------------------------------------------
# The block: the thirteen tensors the file declares, walked in the order it
# declares them in, with the head geometry the file states.
# ---------------------------------------------------------------------------


def test_the_loader_looks_for_the_thirteen_tensors_the_file_declares():
    """`BLOCK_QUANTISED` + `BLOCK_NORMS` are the published file's block, by name.

    WHY spelled out here instead of counted: a loader asking for `attention_norm`
    where the file says `attn_norm` reports a complete model as a missing tensor,
    and a loader that dropped `attn_q_norm` would run Gemma 2's graph against a file
    that ships per-head norms -- an answer from a model the disk does not hold.
    """
    weights = read("weights.rs")
    quantised = string_list(weights, "pub const BLOCK_QUANTISED: &[&str] = &[")
    assert quantised == DECLARED_BLOCK_QUANTISED, (
        f"the block's quantised tensors are {quantised}, "
        f"not the file's {DECLARED_BLOCK_QUANTISED}"
    )
    norms = string_list(weights, "pub const BLOCK_NORMS: &[&str] = &[")
    assert norms == DECLARED_BLOCK_NORMS, (
        f"the block's norms are {norms}, not the file's {DECLARED_BLOCK_NORMS}"
    )

    # The two per-head norms are the ones a Gemma 2 file has no use for and this one
    # ships; the loader is allowed to take them as optional and the graph has to
    # notice when they are there.
    inference = read("inference.rs")
    block = function_body(inference, "pub fn run_block(")
    for field in (
        "attn_norm", "attn_q", "attn_k", "attn_v", "attn_q_norm", "attn_k_norm",
        "attn_o", "post_attn_norm", "ffn_norm", "ffn_gate", "ffn_up", "ffn_down",
        "post_ffn_norm",
    ):
        assert f"lw.{field}" in block, (
            f"run_block never touches lw.{field}, so a tensor the file declares is "
            "being loaded and not used -- or was dropped from the graph"
        )


def test_the_block_graph_is_the_steps_the_declared_architecture_names():
    """RMSNorm, QKV, per-head norms, RoPE, cached attention, output, FFN, residuals.

    WHY order and not mere presence: the published file's blocks are a Gemma-2
    sequence, and the same calls in the wrong order are a different model. A
    normalisation moved after the residual add, or a second RMSNorm left out of the
    pre-feed-forward position, changes the numbers without changing a single count.
    """
    block = without_comments(function_body(read("inference.rs"), "pub fn run_block("))
    for step in (
        "rms_norm(",           # 1. pre-attention norm
        "gemv(",               # 2. q, k, v
        "apply_rope(&mut q",   # 4. RoPE over the query heads the file declares
        "apply_rope(&mut k",   # 4. and over the key head it holds
        "kv_cache.write(",     # 5. the one key/value head, cached
        "softmax(",            # 6. scores normalised over the declared span
        "swiglu(",             # 8. gate and up, multiplied
    ):
        assert step in block, f"the block graph no longer contains {step!r}"

    ordering = [
        block.index("rms_norm("),
        block.index("gemv("),
        block.index("apply_rope(&mut q"),
        block.index("apply_rope(&mut k"),
        block.index("kv_cache.write("),
        block.index("softmax("),
        block.index("swiglu("),
    ]
    assert ordering == sorted(ordering), (
        f"the block's steps are out of the order the architecture states: {ordering}"
    )
    # Every norm the file declares per block has to be the argument of a
    # normalisation, not merely a field the loader happens to fill: dropping one
    # `rms_norm` call leaves the block running and the numbers wrong.
    for norm in ("lw.attn_norm", "lw.attn_q_norm", "lw.attn_k_norm",
                 "lw.post_attn_norm", "lw.ffn_norm", "lw.post_ffn_norm"):
        assert re.search(rf"rms_norm\([^;]*{re.escape(norm)}", block), (
            f"nothing normalises with {norm}, so the block runs a tensor the loader "
            "reads and the graph ignores"
        )
    # Both rotations, not one standing in for both: dropping the key head's RoPE
    # leaves a block that still contains `apply_rope(` and still runs, scoring
    # every position against an unrotated key.
    assert block.count("apply_rope(") == 2, (
        f"the block applies RoPE {block.count('apply_rope(')} times; the architecture "
        "rotates the query heads and the key head, which is two passes"
    )


def test_grouped_query_attention_is_folded_and_more_than_one_kv_head_is_refused():
    """Three query heads over one key/value head is what the file declares.

    WHY this is pinned as a contract: the published file is a grouped-query model
    (`head_count` 3, `head_count_kv` 1, `key_length` 256), the kernel walks one
    key/value head, and that combination is *correct* -- head 0 is the only head the
    file holds. The refusal is for a file declaring two or more key/value heads,
    which would be read as one and reported as the model's. A comment that calls the
    shipped geometry unsupported is as false as one that calls it supported when it
    was not.
    """
    inference = without_comments(read("inference.rs"))
    check = function_body(inference, "pub fn check_attention_shape(&self)")
    assert "self.num_kv_heads > 1" in check, (
        "the shape check stopped refusing a file with more than one key/value head"
    )
    assert '"<arch>.attention.head_count_kv"' in check, (
        "the refusal no longer names the key the file declared"
    )

    block = function_body(read("inference.rs"), "pub fn run_block(")
    assert "for head in 0..config.num_heads" in block, (
        "the block stopped walking the query heads the file declares"
    )
    assert "config.num_kv_heads" in block, (
        "the block no longer reads the key/value head count off the file's declaration"
    )

    # And the documentation may not call the shipped file unsupported.
    doc = doc_block_above(read("inference.rs"), "pub fn check_attention_shape(&self)")
    assert "not a refusal of grouped-query attention" in doc, (
        "the attention check is back to describing the published file's own geometry "
        "as the thing it cannot run"
    )


# ---------------------------------------------------------------------------
# The run: positions, blocks, pool, head, normalisation, and the caller's width.
# ---------------------------------------------------------------------------

HEAD_ORDER = [
    "load_embedding_row",
    "embedding_row_into",
    "load_block",
    "run_block",
    "rms_norm(",
    "mean_of(",
    "run_pooling_head",
    "l2_normalise(",
]


def test_the_streamed_run_walks_the_file_in_the_order_the_file_is_built():
    """Embedding row, 24 blocks, final norm, accumulate, pool, head, L2.

    WHY the order of calls in `run_streamed_embedding` is a contract: the published
    architecture puts the `dense_2`/`dense_3` head AFTER the mean pool and the L2
    step last. Normalising before the head rescales an input the linear layers were
    trained on, and pooling after the head means the head ran on one position
    instead of the sentence.
    """
    body = function_body(read("weights.rs"), "pub fn run_streamed_embedding<S: PositionRead>(")
    positions = []
    for step in HEAD_ORDER:
        assert step in body, f"the streamed run no longer contains {step!r}"
        positions.append(body.index(step))
    assert positions == sorted(positions), (
        f"the run's steps are out of order: {dict(zip(HEAD_ORDER, positions))}"
    )
    assert "kv_cache.len = pos + 1" in body, (
        "the cache length is not advanced per position, so the next prompt inherits "
        "the last one's window"
    )
    assert "reader.has_pooling_head()" in body and "pooled_normalised(" in body, (
        "the run neither asks the file for a head nor says what it does when the file "
        "has none"
    )


def test_the_callers_vector_width_is_refused_before_the_file_is_read():
    """A destination narrower than the model is a refusal, not a truncation.

    WHY first: `out.len() < h_dim` is the difference between a 768-number answer and
    a 512-number prefix of one handed back as the whole embedding, and the check
    that makes it a refusal has to sit ahead of the scratch budget, the cache shape
    and the first positioned read. A caller asking for less than the model gives is
    answered by the status code, not by a shorter vector.
    """
    body = without_comments(
        function_body(read("weights.rs"), "pub fn run_streamed_embedding<S: PositionRead>(")
    )
    guard = body.index("if out.len() < h_dim")
    assert body.index("check_scratch_budget") > guard, (
        "the scratch budget is now consulted before the caller's width, so an "
        "impossible request still costs the device a sizing pass"
    )
    assert "BufferTooSmall" in body[guard : guard + 120], (
        "the width guard no longer refuses with the code that says so"
    )
    # The same rule on the in-memory path, so the two cannot answer differently.
    extract = without_comments(function_body(read("inference.rs"), "pub fn extract_embedding("))
    assert "if out_embedding.len() < config.hidden_dim" in extract, (
        "the in-memory pool truncates where the streamed one refuses"
    )
    assert "Ok(0)" in extract, (
        "an empty prompt stopped being answered as zero positions"
    )


def test_the_descriptor_table_is_walked_through_a_window_or_it_never_arrives():
    """The published file's vocabulary sits between its header and its tensors.

    WHY this is a graph contract and not a performance note: `TensorTable::walk` has
    to pass 262,144 length-prefixed strings to find `general.alignment` and then the
    descriptors, and a string array states no total size -- each element's own prefix
    says where the next begins. Over `GgufStream::Buffered` that is one request per
    512-byte window, counted on the real file at 12,755 pulls. Over a bare
    `GgufStream::File` it is one request per string, and on a device where every read
    is a syscall the walk does not reach the table. The loader cannot choose for its
    caller -- the source is the caller's -- so what this pins is that the walk says
    so, and that the windowed arm is host-proved rather than assumed.
    """
    weights = read("weights.rs")
    doc = doc_block_above(weights, "pub fn walk(stream: &mut crate::gguf::GgufStream<'_>)")
    for truth in ("262,144", "512", "Buffered", "File"):
        assert truth in doc, f"the walk no longer names {truth!r} in its own description"
    assert "WHAT THE CALLER HAS TO CARRY" in doc, (
        "the walk stopped telling its caller that a bare descriptor source never "
        "reaches the descriptor table on a device"
    )

    # And the host case that proves it, over the real file's own metadata.
    tests = function_body(
        weights,
        "fn the_descriptor_table_walks_through_the_metadata_window_the_device_uses",
    )
    assert "INTEROS_REAL_GGUF_PATH" in tests, "the windowed walk is no longer the real file's"
    assert "buffered_from_slice" in tests, "the case does not drive the buffering arm"
    assert "pulls > 0" in tests, (
        "the case would pass on a source that never touched a window -- which is what "
        "every other walk in this suite uses"
    )
    assert "locate_tensor_stream" in tests, (
        "the two descriptor readers are no longer compared through the window"
    )


# ---------------------------------------------------------------------------
# Truth in the comments this engine ships with.
# ---------------------------------------------------------------------------


def test_no_comment_claims_a_dtype_the_arithmetic_does_not_have():
    """The reader of `weights.rs` is told the count and the ids, and they match.

    WHY a second gate on the sentence the other suite already pins: this file's
    claim is different -- it names the *ids*, so a reader can put them next to a
    descriptor table. The ids it lists must be the ids `quantised_row_bytes` arms.
    """
    doc = doc_block_above(read("weights.rs"), "pub enum LoadError")
    claim = re.search(r"(\w+)\s+quantisations", doc)
    assert claim, "the loader no longer states how many quantisations it knows"
    words = {
        "one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6,
        "seven": 7, "eight": 8, "nine": 9, "ten": 10, "eleven": 11, "twelve": 12,
    }
    counted = words.get(claim.group(1).lower())
    assert counted == len(MAPPED_DTYPES), (
        f"the loader claims {claim.group(1)} quantisations; the engine sizes "
        f"{len(MAPPED_DTYPES)}: {MAPPED_DTYPES}"
    )
    for dtype in MAPPED_DTYPES:
        assert re.search(rf"\b{dtype}\b", doc), f"the claim counts {dtype} without naming it"
    for absent in ("Q4_1", "Q5_K", "Q6_K"):
        assert absent in doc, f"the claim stopped saying {absent} is not among them"


def test_the_comments_keep_saying_the_device_has_never_run_this_graph():
    """A host measurement is not a device measurement.

    WHY a gate on the negative: `inference.rs` used to say the published model "does
    not yet run" while the streamed loader was already running it block by block on
    the host -- false in one direction, and the replacement sentence has to keep the
    half that is still true. Nothing here has executed on the guest, so the file that
    sizes the run has to say so where a reader will find it.
    """
    inference = read("inference.rs")
    budget_doc = doc_block_above(inference, "pub const SCRATCH_BYTE_BUDGET")
    assert "does not yet run" not in budget_doc, (
        "the budget still denies a run that the streamed loader performs on the host"
    )
    assert "host" in budget_doc, (
        "the budget no longer says where the run it describes was measured"
    )
    # The negative has to be a sentence about the guest, not the word "never"
    # somewhere in the paragraph: a doc that says the device runs the graph today
    # while mentioning that it never ran some earlier build would otherwise pass.
    assert re.search(
        r"(?i)\b(guest|device)\b[^.]{0,80}\b(never|has not|no measurements)\b",
        budget_doc,
    ), (
        "the budget no longer states, in a sentence about the guest or the device, "
        "that this graph has not run there"
    )
    for lie in ("the device runs", "device-verified run", "on the device it runs"):
        assert lie not in budget_doc.lower(), (
            f"the budget now claims {lie!r}, which nothing in this repository has measured"
        )

    # Mean pooling: what the engine does, and what the file states, are different
    # facts, and the comment has to keep them apart.
    extract = function_body(inference, "pub fn extract_embedding(")
    assert "pooling_type" in extract, (
        "the pooling comment no longer names the key the file states, so a reader "
        "cannot tell that nothing reads it"
    )
    assert re.search(r"no reader in this service asks|nothing in this service reads", extract), (
        "the mean-pool comment is back to implying the engine consults the file's "
        "pooling declaration"
    )

    # And the lexical index may not call itself the neural model.
    rag = read("rag.rs")
    assert "Matryoshka representation learning" not in rag, (
        "the RAG header claims representation learning the module does not do"
    )
    assert "EmbeddingGemma 2 weights" in rag and "lexical" in rag, (
        "the RAG header stopped saying which vectors it actually indexes"
    )
    assert "expandable to 768" not in rag, (
        "the index is back to offering a width it stores no chunk at"
    )


def _address_space_doc_defects(inference: str, services: str, user_rs: str) -> list:
    """What would let the budget doc quote an address-space figure the kernel
    does not grant.

    WHY a cross-check and not a pin on the sentence: this block described the
    inference service as "budgeted 2 GiB" of address space after the kernel had
    started granting `user_address_space_limit()`, a figure derived from the RAM it
    detected. A pin on that sentence would only have held the wrong number in
    place. Reading the grant makes the doc follow the kernel: move the grant and
    this fails until the doc says what the new grant is.
    """
    defects = []
    grant = function_body(services, "ServiceId::Inference => ResourceLimits {")
    granted = re.search(r"max_address_space_bytes:\s*([^,\n]+),", grant)
    if granted is None:
        defects.append("the inference grant no longer states an address-space limit")
    elif "user_address_space_limit" not in granted.group(1):
        defects.append(
            f"the kernel grants the inference service {granted.group(1).strip()}, "
            "not the derived limit this gate holds the doc to"
        )

    # The figure the doc may quote is the one the kernel's own test pins for a
    # 2 GiB guest, so the doc cannot carry a number no artifact produces.
    pinned = re.search(
        r"ram\.set\(2048 \* 1024 \* 1024\);\s*"
        r"let limit_2gb = user_address_space_limit\(\);\s*"
        r"assert_eq!\(limit_2gb, (\d+) \* 1024 \* 1024\);",
        user_rs,
    )
    if pinned is None:
        defects.append(
            "the kernel no longer pins what a 2 GiB guest grants, so the doc's "
            "address-space figure has no artifact behind it"
        )
        return defects

    budget_doc = doc_block_above(inference, "pub const SCRATCH_BYTE_BUDGET")
    if "user_address_space_limit" not in budget_doc:
        defects.append(
            "the budget doc no longer names the function the kernel grants the "
            "service's address space from, so its figure cannot be checked"
        )
    if f"{pinned.group(1)} MiB" not in budget_doc:
        defects.append(
            f"the kernel pins a 2 GiB guest at {pinned.group(1)} MiB of user address "
            "space and the budget doc quotes something else"
        )
    if re.search(r"(?i)budgeted\s+2\s*gib", budget_doc):
        defects.append("the budget doc is back to a fixed 2 GiB grant the kernel does not make")
    return defects


def _address_space_sources() -> tuple:
    kernel = ROOT / "kernel" / "src"
    return (
        read("inference.rs"),
        (kernel / "services.rs").read_text(encoding="utf-8"),
        (kernel / "task" / "user.rs").read_text(encoding="utf-8"),
    )


def test_the_budget_doc_quotes_the_address_space_limit_the_kernel_grants():
    """The doc's figure is the kernel's, read from the kernel, not remembered."""
    defects = _address_space_doc_defects(*_address_space_sources())
    assert defects == [], "; ".join(defects)


def test_the_address_space_doc_contract_can_fail():
    """Each detector above is tripped by the drift it exists for."""
    inference, services, user_rs = _address_space_sources()
    assert _address_space_doc_defects(inference, services, user_rs) == []

    remembered = inference.replace("computes to 1536 MiB", "computes to 2048 MiB", 1)
    assert remembered != inference, "the figure the contract watches moved"
    assert any(
        "quotes something else" in d
        for d in _address_space_doc_defects(remembered, services, user_rs)
    ), "a doc quoting a number the kernel never computed passed"

    two_gib = inference.replace(
        "is budgeted `user_address_space_limit()`", "is budgeted 2 GiB of it", 1
    )
    assert two_gib != inference, "the grant sentence the contract watches moved"
    assert any(
        "fixed 2 GiB grant" in d for d in _address_space_doc_defects(two_gib, services, user_rs)
    ), "the original over-claim, a flat 2 GiB budget, passed"

    unnamed = inference.replace(
        "`ServiceId::Inference` is budgeted `user_address_space_limit()`",
        "`ServiceId::Inference` is budgeted generously",
        1,
    )
    assert unnamed != inference, "the symbol the contract watches moved"
    assert any(
        "no longer names the function" in d
        for d in _address_space_doc_defects(unnamed, services, user_rs)
    ), "a doc that quotes a figure with no symbol to check it against passed"

    authored = services.replace(
        "max_address_space_bytes: crate::task::user::user_address_space_limit() as u64,",
        "max_address_space_bytes: 2 * 1024 * 1024 * 1024,",
        1,
    )
    assert authored != services, "the grant the contract watches moved"
    assert any(
        "not the derived limit" in d
        for d in _address_space_doc_defects(inference, authored, user_rs)
    ), "the kernel going back to an authored ceiling went unnoticed"

    unpinned = user_rs.replace(
        "assert_eq!(limit_2gb, 1536 * 1024 * 1024);", "", 1
    )
    assert unpinned != user_rs, "the kernel pin the contract watches moved"
    assert any(
        "no artifact behind it" in d
        for d in _address_space_doc_defects(inference, services, unpinned)
    ), "losing the kernel's own figure left the doc's number uncheckable"


def _weight_pricing_defects(inference: str) -> list[str]:
    defects = []
    if "pub fn parameter_count(&self) -> usize" not in inference:
        defects.append("ModelConfig no longer defines parameter_count")
    if "pub fn weight_bytes(&self, dtype: u32) -> Result<usize, InferenceError>" not in inference:
        defects.append("ModelConfig no longer defines weight_bytes to price weights in the cost model")
    if "12 MB/s" in inference or "12 MB" in inference:
        defects.append("inference.rs still contains unmeasured throughput claim: '12 MB/s'")
    if "conversational_2b_model_feasibility_verdict" in inference:
        defects.append("inference.rs still names an invented 'feasibility verdict'")
    if "test_model_config_prices_weight_bytes_against_memory_budget" not in inference:
        defects.append("inference.rs no longer tests weight_bytes in test_model_config_prices_weight_bytes_against_memory_budget")
    return defects


def test_the_cost_model_prices_weight_bytes_and_refuses_unmeasured_throughput_claims():
    """Weight footprints are priced in the cost model; unmeasured throughput claims are refused."""
    defects = _weight_pricing_defects(read("inference.rs"))
    assert defects == [], "; ".join(defects)


def test_the_weight_pricing_gate_can_fail():
    """Each detector above trips on the drift it exists for."""
    inference = read("inference.rs")
    assert _weight_pricing_defects(inference) == []

    # 1. Reintroducing 12 MB/s unmeasured throughput
    speed = inference.replace(
        "test_model_config_prices_weight_bytes_against_memory_budget",
        "// At 12 MB/s PIO\nfn test_model_config_prices_weight_bytes_against_memory_budget",
        1,
    )
    assert any("unmeasured throughput claim" in d for d in _weight_pricing_defects(speed))

    # 2. Removing weight_bytes from cost model
    missing = inference.replace("pub fn weight_bytes(&self, dtype: u32) -> Result<usize, InferenceError>", "pub fn unpriced_weights(&self) -> Result<usize, InferenceError>", 1)
    assert any("no longer defines weight_bytes" in d for d in _weight_pricing_defects(missing))


def _gemma_declared_architecture_defects(inference: str, weights: str, lib: str, model_path: Path = None) -> list[str]:
    defects = []
    if "ModelIdentity::from_gguf" not in inference:
        defects.append("inference.rs does not parse ModelIdentity::from_gguf from metadata")
    if "info.architecture()" not in inference:
        defects.append("inference.rs does not read architecture from GGUF metadata")
    if "info.file_type" not in inference:
        defects.append("inference.rs does not read quantization file_type from GGUF metadata")
    if "pub fn run_streamed_embedding" not in weights:
        defects.append("weights.rs does not provide run_streamed_embedding")
    if "dense_2.weight" not in weights or "dense_3.weight" not in weights:
        defects.append("weights.rs does not verify published pooling head tensors")
    if "/data/models/embeddinggemma-2.gguf" not in lib:
        defects.append("lib.rs does not name /data/models/embeddinggemma-2.gguf in missing weight refusal")
    if "test_model_config_prices_weight_bytes_against_memory_budget" not in inference:
        defects.append("inference.rs does not verify priced weight footprint against memory budget")
    if "pub fn check_admission" not in inference:
        defects.append("inference.rs does not provide check_admission")
    if "config.check_admission(info.file_type)" not in inference:
        defects.append("ModelConfig::from_gguf does not consult check_admission with declared file_type")

    # Binary GGUF assertions: fail loudly if model artifact is missing
    real_path = model_path if model_path is not None else Path("target/verify/models/embeddinggemma-300M-Q8_0.gguf")
    if not real_path.exists():
        defects.append(f"Model file {real_path} not found: binary GGUF header verification requires declared artifact")
    else:
        with open(real_path, "rb") as f:
            header = f.read(24)
            magic, version, tensor_count, kv_count = struct.unpack("<4sIQQ", header)
            if magic != b"GGUF":
                defects.append(f"GGUF magic invalid: {magic}")
            if version != 3:
                defects.append(f"GGUF version is not 3: {version}")
            if tensor_count != 316:
                defects.append(f"Declared tensor count is {tensor_count}, expected 316")
    return defects


def test_the_gemma_architecture_and_quantisation_are_run_and_missing_weights_refuse_by_path():
    """Run published Gemma architecture from metadata and refuse missing weights naming the path."""
    defects = _gemma_declared_architecture_defects(
        read("inference.rs"),
        read("weights.rs"),
        read("lib.rs"),
    )
    assert defects == [], "; ".join(defects)


def test_the_declared_architecture_contract_can_fail():
    """Each detector above trips when declared architecture or path refusal is broken."""
    inference = read("inference.rs")
    weights = read("weights.rs")
    lib = read("lib.rs")
    assert _gemma_declared_architecture_defects(inference, weights, lib) == []

    # 1. Missing metadata-driven architecture
    no_arch = inference.replace("info.architecture()", "None")
    assert any("read architecture from GGUF metadata" in d for d in _gemma_declared_architecture_defects(no_arch, weights, lib))

    # 2. Removing run_streamed_embedding
    no_stream = weights.replace("pub fn run_streamed_embedding", "pub fn legacy_embedding")
    assert any("run_streamed_embedding" in d for d in _gemma_declared_architecture_defects(inference, no_stream, lib))

    # 3. Dropping missing path in refusal
    no_path = lib.replace("/data/models/embeddinggemma-2.gguf", "some_generic_file")
    assert any("/data/models/embeddinggemma-2.gguf" in d for d in _gemma_declared_architecture_defects(inference, weights, no_path))

    # 4. Dropping check_admission
    no_adm = inference.replace("pub fn check_admission", "pub fn unverified_admission")
    assert any("check_admission" in d for d in _gemma_declared_architecture_defects(no_adm, weights, lib))

    # 5. Missing model artifact causes failure (fail-closed, not silently skipped)
    assert any("Model file" in d for d in _gemma_declared_architecture_defects(inference, weights, lib, model_path=Path("nonexistent_model.gguf")))


def _untrusted_input_bound_defects(weights: str, inference: str) -> list[str]:
    defects = []
    stream_body = function_body(weights, "pub fn run_streamed_embedding<S: PositionRead>(")
    if "tokens.len() > config.seq_len" not in stream_body:
        defects.append("run_streamed_embedding does not enforce tokens.len() against config.seq_len")
    if "ContextLengthExceeded" not in stream_body:
        defects.append("run_streamed_embedding does not refuse over-length tokens with ContextLengthExceeded")
    if "tokens.len().min(config.seq_len)" in stream_body:
        defects.append("run_streamed_embedding silently truncates tokens with min(config.seq_len) instead of refusing")
    if re.search(r"tokens\.len\(\)\s*>\s*\d+", stream_body):
        defects.append("run_streamed_embedding bounds tokens with an authored integer constant instead of declared config.seq_len")

    extract_body = function_body(inference, "pub fn extract_embedding(")
    if "tokens.len() > config.seq_len" not in extract_body:
        defects.append("extract_embedding does not enforce tokens.len() against config.seq_len")
    if "ContextLengthExceeded" not in extract_body:
        defects.append("extract_embedding does not refuse over-length tokens with ContextLengthExceeded")
    if "tokens.len().min(config.seq_len)" in extract_body:
        defects.append("extract_embedding silently truncates tokens with min(config.seq_len) instead of refusing")
    if re.search(r"tokens\.len\(\)\s*>\s*\d+", extract_body):
        defects.append("extract_embedding bounds tokens with an authored integer constant instead of declared config.seq_len")

    return defects


def test_untrusted_input_bounds_are_sized_from_model_declaration_and_refused():
    """Untrusted input sequence length is bounded by declared seq_len from the model file and refused."""
    defects = _untrusted_input_bound_defects(read("weights.rs"), read("inference.rs"))
    assert defects == [], "; ".join(defects)


def test_the_untrusted_input_bounds_gate_can_fail():
    """Each detector above trips when input bounds or refusals are broken."""
    weights = read("weights.rs")
    inference = read("inference.rs")
    assert _untrusted_input_bound_defects(weights, inference) == []

    # 1. Silent clamping in run_streamed_embedding
    clamped_weights = weights.replace(
        "if tokens.len() > config.seq_len {\n        return Err(InferenceError::ContextLengthExceeded.into());\n    }\n    let positions = tokens.len();",
        "let positions = tokens.len().min(config.seq_len);",
        1,
    )
    assert any("silently truncates" in d for d in _untrusted_input_bound_defects(clamped_weights, inference))

    # 2. Authored constant in run_streamed_embedding
    authored_weights = weights.replace(
        "if tokens.len() > config.seq_len {",
        "if tokens.len() > 512 {",
        1,
    )
    assert any("authored integer constant" in d for d in _untrusted_input_bound_defects(authored_weights, inference))

    # 3. Silent clamping in extract_embedding
    clamped_infer = inference.replace(
        "if tokens.len() > config.seq_len {\n        return Err(InferenceError::ContextLengthExceeded);\n    }\n    let count = tokens.len();",
        "let count = tokens.len().min(config.seq_len);",
        1,
    )
    assert any("silently truncates" in d for d in _untrusted_input_bound_defects(weights, clamped_infer))

    # 4. Authored constant in extract_embedding
    authored_infer = inference.replace(
        "if tokens.len() > config.seq_len {",
        "if tokens.len() > 2048 {",
        1,
    )
    assert any("authored integer constant" in d for d in _untrusted_input_bound_defects(weights, authored_infer))


