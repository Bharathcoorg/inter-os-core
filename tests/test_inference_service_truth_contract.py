# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Truth contract for the on-device inference service's replies.

The service answered a chat prompt with words from weights it never loaded: a
forward pass over zero-filled tensors at the dimensions a unit test had picked,
reported with STATUS_SUCCESS. Every assertion below pins one half of the rule
that replaces it -- the model file on disk is the only thing allowed to size the
engine, and when the engine cannot be built from that file the reply says so --
and one half of the rule that carries any reply bigger than a mailbox to the
reader that asked for it, whole or reported short -- and one half of the rule
that lets a model file reach the device at all: the copy is made by the service
that already holds the write, from a tree the platform names, and the answer is
a measurement of what arrived.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SERVICE = ROOT / "userspace" / "system-service" / "src"
KERNEL_SERVICES = ROOT / "kernel" / "src" / "services.rs"

GENERATIVE_START = "// 2. Generative token stream queries"
GENERATIVE_END = "// 3. Semantic Search / RAG document retrieval query"

# What a fabricated forward pass is made of: a graph assembled in the dispatch
# arm, weights with no file behind them, and the fixture that supplied them.
FABRICATED = (
    "inference::generate(",
    "ModelWeights",
    "LayerWeights",
    "default_test_config(",
    "ones_norm",
    "id_w",
    "ffn_gate_w",
    "head_w",
    "decoded_chunk",
)


def read(name: str) -> str:
    return (SERVICE / name).read_text(encoding="utf-8")


def lib_source() -> str:
    return read("lib.rs")


def generative_arm(source: str) -> str:
    """The dispatch arm that answers a GENR/GENERATE query, and nothing else."""
    start = source.index(GENERATIVE_START)
    return source[start : source.index(GENERATIVE_END, start)]


def without_comments(arm: str) -> str:
    """`arm` with its comments dropped.

    A comment naming a synthetic tensor is not the arm building one, so the
    detectors for fabricated weights run over the code alone.
    """
    kept = []
    for line in arm.splitlines():
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        kept.append(line.split("//")[0])
    return "\n".join(kept)


def flat(source: str) -> str:
    """`source` with every run of whitespace collapsed to a single space.

    WHY: a fixture's bytes are the contract, but rustfmt owns how they are laid
    out, and a pin written against one line of a wrapped call fails for a reason
    that has nothing to do with what the fixture declares.
    """
    return " ".join(source.split())


def function_body(source: str, signature: str) -> str:
    """The body of the function whose line starts with `signature`."""
    start = source.index(signature)
    brace = source.index("{", start)
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[brace:index]
    raise AssertionError(f"unbalanced braces after {signature!r}")


def doc_comment(source: str, signature: str) -> str:
    """The `///` block sitting directly above the line starting with `signature`."""
    start = source.index(signature)
    head = source[:start].rstrip("\n")
    lines = []
    for line in reversed(head.splitlines()):
        if line.strip().startswith("///"):
            lines.append(line)
            continue
        break
    return "\n".join(reversed(lines))


def policy_arm(services: str, name: str) -> str:
    """The capability list the kernel's service policy gives to service `name`.

    WHY it is read out on its own: an install has to be served by the authority
    that already exists, so the contract has to look at one service's grant list
    rather than at the file, where a widened grant would read as ordinary code.
    """
    start = services.index(f"ServiceId::{name} => {{")
    brace = services.index("{", start)
    depth = 0
    for index in range(brace, len(services)):
        if services[index] == "{":
            depth += 1
        elif services[index] == "}":
            depth -= 1
            if depth == 0:
                return services[start:index]
    raise AssertionError(f"unbalanced braces in the {name} policy arm")


def test_the_walk_counts_a_vocabulary_it_refuses_to_load():
    """The engine needs a vocabulary's width, not its 256,000 strings.

    A real Gemma array is sixty times the reader's load cap, so the cap cannot
    be raised and the array cannot be held. The walk takes the element count
    from the array's own header, records it as counted-not-loaded, and says in
    the notice that the number did not come from bytes it read.
    """
    gguf = read("gguf.rs")
    assert "pub const MAX_TOKENIZER_PRE_LEN: usize = 32;" in gguf
    assert "pub const MAX_CHAT_TEMPLATE_LEN: usize = 1024;" in gguf
    assert "pub const METADATA_WINDOW_BYTES: usize = 512;" in gguf
    assert "pub enum VocabReading {" in gguf
    for outcome in (
        "Absent,",
        "Counted { elements: u32 },",
        "BeyondCount { stated: u64 },",
        "Truncated { stated: u32 },",
    ):
        assert outcome in gguf, f"VocabReading no longer separates {outcome!r}"
    assert "pub fn vocab_counted_not_loaded(&self) -> bool" in gguf
    # The count comes from the array header, and the elements are stepped over by
    # their length prefixes -- never copied into anything.
    stepper = function_body(gguf, "fn step_over_string_array")
    assert "stream.read_u64()" in stepper
    assert "stream.skip(" in stepper
    for heap in ("Vec::", "String::", "to_owned(", "copy_from_slice"):
        assert heap not in stepper, f"the vocabulary step loads bytes with {heap!r}"


def test_a_key_declared_after_the_array_is_still_reached():
    """`tokenizer.chat_template` sits past the vocabulary in a real Gemma file.

    WHY the walk compares whole keys: `metadata_field` matches only the last
    dotted segment, so a file carrying `some.other.pre` would otherwise set the
    tokenizer's family name. And WHY a second walk at all: the configuration
    walk stops at the vocabulary by design, so the template needs a pass that
    carries on past it.
    """
    gguf = read("gguf.rs")
    walk = function_body(gguf, "fn walk_tokenizer_kv")
    for key in (
        'b"tokenizer.ggml.tokens"',
        'b"tokenizer.ggml.bos_token_id"',
        'b"tokenizer.ggml.eos_token_id"',
        'b"tokenizer.ggml.padding_token_id"',
        'b"tokenizer.ggml.add_bos_token"',
        'b"tokenizer.ggml.pre"',
        'b"tokenizer.chat_template"',
    ):
        assert key in gguf, f"the walk no longer names {key}"
    assert "key == KEY_" in walk, "the walk matched a key by suffix instead of whole"
    assert "metadata_field(" not in walk, "a suffix match would let another family set these"
    # The count is taken, the array stepped over, and the loop carries on.
    assert walk.index("VocabReading::Counted { elements }") < walk.index(
        "step_over_string_array(stream, elements as u64)"
    )
    assert walk.index("step_over_string_array(stream, elements as u64)") < walk.index(
        "key == KEY_CHAT_TEMPLATE"
    )
    assert "pub fn read_tokenizer_info(path: &str) -> Result<GgufTokenizerInfo, GgufError>" in gguf
    body = function_body(gguf, "pub fn read_tokenizer_info(path:")
    assert "GgufStream::buffered_from_file(fd)" in body
    assert "std::fs_close(fd)" in body, "the walk leaks the model file's descriptor"


def test_a_file_that_ends_inside_the_array_reports_the_key_not_a_panic():
    """A truncated file still declared what it got to.

    WHY running out is an `Ok`: the walk's job is to say what the file stated,
    and a file that stopped mid-vocabulary stated a count before it stopped. The
    reader names the key it ran out inside and keeps every key it had reached;
    it does not fail the whole read, because the engine's next move with a
    half-file is to say which half it has.
    """
    gguf = read("gguf.rs")
    assert "pub truncated_in: Option<&'static [u8]>," in gguf
    reader = function_body(gguf, "pub fn read_tokenizer_info_stream")
    assert "Err(GgufError::BufferTooShort) | Err(GgufError::ReadFailed) => {}" in reader
    assert "unwrap()" not in reader and "panic!" not in reader
    walk = function_body(gguf, "fn walk_tokenizer_kv")
    assert "$info.truncated_in = Some($key)" in gguf, "the pair macro lost the key's name"
    assert "info.truncated_in = Some(KEY_TOKENS)" in walk
    assert "VocabReading::Truncated {" in walk
    # A read that ran out inside the vocabulary still reports the count the file
    # gave, so the engine is not told a truncated file declared no vocabulary.
    assert "stated: elements," in walk


def test_an_over_long_template_is_refused_whole_by_the_length_the_file_stated():
    """A 4000-byte template must not be answered as a 1024-byte one.

    Silently cutting a template to the reader's window would run the model's own
    formatting wrong and report it as the file's. The reader refuses by the
    length the file declares, before copying a byte, and reports that number.
    """
    gguf = read("gguf.rs")
    assert "RefusedTooLong(u64)," in gguf
    body = function_body(gguf, "fn read_bounded_string")
    assert body.index("if need > dst.len()") < body.index("stream.read_exact("), (
        "the reader copied into the window before it checked the length"
    )
    assert "return Ok(StringReading::RefusedTooLong(stated))" in body
    # A refused value is stepped past, so the walk reaches the keys after it.
    assert "stream.skip(need)" in body


def test_a_flag_is_read_as_the_one_byte_gguf_type_seven_is():
    """`add_bos_token` is a bool: type 7, one byte.

    WHY it is its own reader: `read_epsilon` accepts only the two float widths
    and `read_count` only the integer ones, so a bool routed through either is
    either rejected out of hand or accepted at any width. And a byte that is
    neither 0 nor 1 is not a declared flag, so the file gets said so.
    """
    gguf = read("gguf.rs")
    bool_reader = function_body(gguf, "fn read_bool")
    assert "if val_type != 7" in bool_reader
    assert "let mut byte = [0u8; 1];" in bool_reader
    assert "0 => Ok(false)," in bool_reader
    assert "1 => Ok(true)," in bool_reader
    assert "_ => Err(GgufError::InvalidMetadata)," in bool_reader
    assert "read_bool(stream, val_type)" in function_body(gguf, "fn walk_tokenizer_kv")
    # The float reader was not widened to take a bool: it still names only 6 and 12.
    epsilon = function_body(gguf, "fn read_epsilon")
    assert "6 =>" in epsilon and "12 =>" in epsilon
    assert "7 =>" not in epsilon


def test_a_file_with_no_tokenizer_block_loads_and_says_the_default_runs():
    """A stripped-vocabulary embedding file has no tokenizer, and still loads.

    WHY "absent" is not "empty": the platform has a built-in template, so the
    reader has to tell a file that named none from one that named an empty
    string -- and name which template then formatted the prompt.
    """
    gguf = read("gguf.rs")
    assert "pub fn declares_no_tokenizer(&self) -> bool" in gguf
    assert "pub enum TemplateSource {" in gguf
    assert "BuiltInDefault," in gguf
    assert 'pub const DEFAULT_CHAT_TEMPLATE: &str =' in gguf
    assert "PROMPT_MARKER: &[u8] = b\"{prompt}\";" in gguf
    # The seven keys the architecture work already made mandatory stay the only
    # seven. Making a tokenizer string or id mandatory here would turn a file the
    # engine loads today -- a stripped embedding model, an empty placeholder --
    # into one it refuses, and would take `ModelGap::NoTensors` away.
    missing = function_body(gguf, "pub fn missing_key")
    assert missing.count("return Some(") == 7, (
        "missing_key demands a key the engine did not require before this walk"
    )
    for added in ("chat_template", "bos_token", "eos_token", "padding_token", "add_bos", ".pre"):
        assert added not in missing, f"missing_key now demands the new key {added!r}"
    lib = lib_source()
    assert ", tokenizer absent, built-in template in use" in lib


def test_the_notice_carries_what_the_walk_read_counted_and_refused():
    """The same status line that quotes the architecture now quotes the tokenizer.

    WHY clauses are statements about the file: "template 1024 bytes from file",
    "template 4096 bytes refused" and "template built-in default" are three
    different facts about three different files, and only the first is the
    model's own formatting.
    """
    lib = lib_source()
    clause = function_body(lib, "fn tokenizer_clause")
    for phrase in (
        ", vocabulary counted not loaded",
        " bytes from file",
        ", template built-in default",
        " bytes refused",
        ", no token ids",
        ", add-bos 1",
        ", pre ",
        ", file ended inside ",
    ):
        assert phrase in clause, f"the walk no longer says {phrase!r}"
    arm = generative_arm(lib)
    assert "gguf::read_tokenizer_info(path)" in arm
    assert "tokenizer.as_ref()" in arm
    # Every arm writes into the one window the service names. WHY this replaced the
    # per-arm literals (448 for the walk, 320 for the empty file, 240 twice for the
    # refusals the walk cannot speak about): each literal had been measured against
    # the sentence a test fixture produced, not the widest sentence the reader can
    # be handed, and Notice cuts mid-word once its window is full. The measurement
    # now lives in the Rust guard it is named after.
    #
    # WHY the count is five and not the four it was: the generative route grew a
    # refusal of its own -- the prompt that did not all arrive, named by both byte
    # counts before the parse clamps it to the prefix -- and that sentence is wider
    # than the forty bytes an inline reply keeps, so it takes the same window as the
    # others. An arm that reached for its own narrower buffer is what the `stale`
    # loop below refuses.
    assert arm.count("let mut scratch = [0u8; NOTICE_WINDOW_BYTES];") == 5
    for stale in ("[0u8; 448]", "[0u8; 320]", "[0u8; 240]", "[0u8; 512]", "[0u8; 704]"):
        assert stale not in arm, f"an arm kept its own narrower window {stale!r}"
    assert "const NOTICE_WINDOW_BYTES: usize = 768;" in lib


def test_the_real_shape_is_proved_where_a_256k_array_can_be_built():
    """A committed Python test cannot hold a 256k-entry array.

    `target/verify/` is gitignored, so the byte-level proof that the walk passes
    a real Gemma vocabulary and reaches the key after it lives as a host-side
    Rust test that builds the shape itself. This case pins that the proof still
    has a name, so a future change cannot delete the only test that exercised
    the array.
    """
    gguf = read("gguf.rs")
    for proof in (
        "fn test_tokenizer_walk_counts_a_vocabulary_far_beyond_its_own_load_cap",
        "fn test_tokenizer_walk_reports_a_file_that_ends_inside_the_vocabulary",
        "fn test_tokenizer_walk_refuses_a_template_over_its_cap_with_the_length_it_stated",
        "fn test_tokenizer_walk_reads_a_flag_as_bool_type_7_and_refuses_any_other_width",
        "fn test_tokenizer_walk_reports_a_file_that_declares_no_tokenizer_but_still_loads",
        "fn test_tokenizer_walk_names_its_keys_whole_and_ignores_another_family",
        "fn test_prompt_formatting_splices_at_the_marker_and_reports_why_it_could_not",
    ):
        assert proof in gguf, f"the walk's real-shape proof {proof!r} is gone"
    assert "256_000" in gguf, "no test builds a vocabulary past the load cap any more"


def test_the_generative_arm_answers_nothing_with_success():
    """The arm used to reply STATUS_SUCCESS from weights it had not loaded."""
    arm = generative_arm(lib_source())
    assert "std::ai::STATUS_SUCCESS" not in arm
    for status in ("std::ai::STATUS_UNSUPPORTED", "std::ai::STATUS_UNAVAILABLE"):
        assert status in arm, f"the arm no longer reports {status}"


def test_the_generative_arm_builds_no_forward_pass():
    """No tensor, cache or generate call may be assembled in the dispatch arm.

    An engine call belongs behind weights the service has actually read; until
    it does, the arm only reports what stopped it.
    """
    code = without_comments(generative_arm(lib_source()))
    for fabricated in FABRICATED:
        assert fabricated not in code, f"the generative arm builds {fabricated!r}"


def test_the_engine_is_sized_from_the_model_file():
    """The dimensions come from the GGUF metadata, not from a test fixture."""
    arm = generative_arm(lib_source())
    assert "gguf::read_model_info(path)" in arm
    assert "inference::ModelConfig::from_gguf(&info)" in arm
    assert "pub fn from_gguf(info: &crate::gguf::GgufModelInfo)" in read("inference.rs")
    # The fixture is still the engine's own test helper, and the service's
    # dispatch code never reaches for it.
    assert "fn default_test_config() -> Self" in read("inference.rs")
    assert "default_test_config" not in arm


def test_a_declaration_beyond_the_engine_is_refused_not_clamped():
    """A 2304-wide Gemma must not be run as a 256-wide model.

    Clamping the file's dimensions to the engine's bounds would answer with a
    graph that is neither the model nor a refusal.
    """
    inference = read("inference.rs")
    assert "ModelDeclaration(" in inference
    assert "fn fits(" in inference
    # WHY the whole signature and not the bare name: this file now carries a second
    # `pub fn from_gguf` -- the one that builds the identity clause -- and
    # `function_body` reads the first match it finds. Pointed at the name alone,
    # this guard would have spent the last change checking a builder that holds no
    # dimensions for clamps, and reported nothing.
    body = function_body(
        inference,
        "pub fn from_gguf(info: &crate::gguf::GgufModelInfo) -> Result<Self, InferenceError>",
    )
    assert "hidden_dim" in body, "the guard is reading a function that prices no dimensions"
    for clamp in (".min(", ".max(", "unwrap_or(MAX_"):
        assert clamp not in body, f"from_gguf clamps a dimension with {clamp!r}"
    # The size answer is one computed byte budget, not a per-dimension ceiling:
    # the six compile-time maxima refused every published model, because every one
    # of them exceeds at least one of those ceilings while the device has room for
    # the shape. What bounds a run now is what it costs.
    assert "check_scratch_budget" in body, "from_gguf no longer prices the declared shape"
    assert "ScratchTooLarge(needed, SCRATCH_BYTE_BUDGET)" in inference
    # The ceilings are gone from the engine's code. Prose is allowed to name one
    # to explain what the budget replaced; a declaration, or a check reading a
    # dimension against it, is not.
    code = without_comments(inference)
    for ceiling in (
        "MAX_HIDDEN_DIM",
        "MAX_INTERMEDIATE_DIM",
        "MAX_LAYERS",
        "MAX_SEQ_LEN",
        "MAX_VOCAB_SIZE",
        "MAX_HEAD_DIM",
    ):
        assert ceiling not in code, f"{ceiling} is back as a compile-time ceiling"
    assert "const MAX_" not in inference, "a compile-time ceiling has come back"
    assert "MAX_HEAD_DIM" not in read("lib.rs"), "the ceilings are back in the dispatch arm"


def test_the_run_is_priced_before_any_memory_is_asked_for():
    """The budget is checked, then the heap is asked: never the other way round.

    A shape the device cannot hold has to be refused on the arithmetic, and the
    refusal has to carry both numbers, or the next stage is guessing whether it
    needs a smaller model or a bigger machine.
    """
    inference = read("inference.rs")
    assert "pub const SCRATCH_BYTE_BUDGET: usize = 120 * 1024 * 1024;" in inference
    for priced in ("pub fn kv_cache_bytes", "pub fn scratch_bytes", "fn check_scratch_budget"):
        body = function_body(inference, priced)
        assert body, f"the engine no longer computes {priced!r}"
    for entry in ("pub fn generate", "pub fn extract_embedding"):
        body = function_body(inference, entry)
        assert "config.check_scratch_budget()?" in body, f"{entry} allocates before pricing"
        assert body.index("check_scratch_budget") < body.index("Scratch::new"), (
            f"{entry} asks the heap for the run before it checks the run fits"
        )
    # The scratch and the cache live on the service's memory capability, not on
    # the 256 KiB user stack a service thread is given.
    assert "use alloc::vec::Vec;" in inference
    assert ".try_reserve_exact(" in inference, "the scratch falls back to the aborting allocator"
    assert "Vec<f32>" in inference, "the scratch buffers are fixed arrays again"
    assert "pub struct Scratch" in inference and "pub struct KvCache" in inference


def test_the_scratch_is_taken_from_the_memory_the_service_is_already_granted():
    """96 MiB of cache needs a mapping, and one exists only behind a capability.

    The change is not "use malloc": it is using the memory the kernel already
    handed this service and nobody else, which is why the allocator declaration,
    the crate that needs a heap and the kernel's grant list are pinned together.
    A `#[global_allocator]` applies to every binary that links an allocating
    library, so it lives once in the library -- putting it in `aid.rs` alone would
    either fail to link the other ten daemons or quietly give them a heap too.
    """
    lib = read("lib.rs")
    assert "extern crate alloc;" in lib, "the library asks for a heap it never declared"
    assert "#[global_allocator]" in lib
    assert "interos_std::heap::InterOsHeap" in lib, "the scratch is not on the service's heap"
    assert "#[cfg(not(test))]" in lib, (
        "the host suite would run against the syscall-backed heap it cannot call"
    )
    aid = (SERVICE / "bin" / "aid.rs").read_text(encoding="utf-8")
    assert "#[global_allocator]" not in without_comments(aid), (
        "two allocator declarations, one per binary"
    )
    assert "#![no_std]" in aid, "the service binary stopped being freestanding"
    services = KERNEL_SERVICES.read_text(encoding="utf-8")
    assert "CapabilityType::MemoryHeap" in policy_arm(services, "Inference"), (
        "inference was given a heap it has no capability for"
    )


def test_the_real_model_file_is_read_or_its_absence_is_said_aloud():
    """The one case that opens the 333 MB file has to announce when it did not.

    Every other case prices a description of the file typed in by hand, so a
    reader wrong about the real bytes passes the whole suite as long as the
    fixture repeats the mistake. This test exists to keep the file-reading case
    honest: gated on a path it prints, running the shipping reader and the
    shipping pricing, and refusing the same file at a longer context so the
    budget cannot be read as "tuned until EmbeddingGemma fit".
    """
    inference = read("inference.rs")
    body = function_body(
        inference, "fn test_the_real_published_file_is_priced_and_allocated"
    )
    assert "INTEROS_REAL_GGUF_PATH" in body, "the real file is no longer opt-in, or is silent"
    assert "NOT VERIFIED" in body, (
        "a run with no file in front of it would pass without saying it read nothing"
    )
    for shipping in (
        "crate::gguf::read_model_info_stream",
        "ModelConfig::from_gguf",
        "Scratch::new(&config)",
        "KvCache::new(&config)",
        "check_scratch_budget",
    ):
        assert shipping in body, f"the real-file case no longer runs {shipping!r}"
    assert "ScratchTooLarge(" in body, "the real file is only ever shown being accepted"


def test_the_head_geometry_is_checked_against_the_row_it_is_cached_in():
    """Each head is cached in a row `head_dim` wide, and the heads are laid end
    to end across the hidden dimension.

    The row width is the file's number now, so what is checked is the geometry --
    heads that do not tile their own residual stream, and more key/value heads
    than the attention kernel walks -- not a compiled-in ceiling.
    """
    inference = read("inference.rs")
    body = function_body(inference, "pub fn check_attention_shape")
    assert body, "the head geometry is no longer checked at all"
    assert "heads_span" in body
    assert "attention.head_count x key_length" in body
    assert "attention.head_count_kv" in body, (
        "a grouped-query file is no longer refused by the kernel's own reason"
    )


def test_the_head_width_a_published_file_names_is_read_not_missed():
    """GGUF writers spell one head's width two ways, and both are the same number.

    The published embedding model states `gemma-embedding.attention.key_length`
    256. Asking only for `head_length` made a complete file look like one missing
    a key, and the service told the person so -- the failure mode this suite
    exists to catch is a notice that misdescribes the file in front of it.
    """
    gguf = read("gguf.rs")
    walk = function_body(gguf, "pub fn read_model_info_stream")
    assert 'b"key_length" | b"head_length"' in walk, "the reader stopped taking one spelling"
    assert 'Some("<arch>.attention.key_length")' in gguf
    fixture = flat(function_body(
        gguf, "fn test_read_model_info_configures_a_published_embedding_model"
    ))
    assert fixture, "the published file's key shape is no longer a test"
    # Every number the run is priced from, exactly as the real file states them:
    # the cache is deep as `context_length` and narrow as `head_count_kv`, so a
    # fixture that drops either is no longer the published file.
    for stated in (
        'b"gemma-embedding.attention.key_length", 256',
        'b"gemma-embedding.attention.head_count_kv", 1',
        'b"gemma-embedding.context_length", 2048',
        "262_144",
    ):
        assert stated in fixture, f"the fixture lost the real file's {stated!r}"


def test_the_published_head_width_rule_can_fail():
    """Either half of the rule above has to be trippable."""
    gguf = read("gguf.rs")

    narrowed = gguf.replace('b"key_length" | b"head_length" =>', 'b"head_length" =>')
    assert narrowed != gguf, "the spelling fixture no longer matches the reader"
    assert 'b"key_length" | b"head_length"' not in function_body(
        narrowed, "pub fn read_model_info_stream"
    ), "a reader that takes only one spelling passed"

    silent = gguf.replace(
        '            return Some("<arch>.attention.key_length");',
        '            return Some("<arch>.attention.head_length");',
    )
    assert silent != gguf, "the missing-key fixture no longer matches"
    assert 'Some("<arch>.attention.key_length")' not in silent


def test_the_reader_refuses_to_walk_a_vocabulary_to_reach_a_later_key():
    """A 256k-token array is not 256k syscalls.

    Every read of a device-backed file remounts and rescans the inode table, so
    the reader takes the element count from the array's own header and stops,
    naming the key it did not reach.
    """
    gguf = read("gguf.rs")
    assert "pub const MAX_STRING_ARRAY_ELEMENTS: usize = 4096;" in gguf
    assert "MissingMetadata(&'static str, GgufModelInfo)" in gguf
    assert "fn missing_key(&self) -> Option<&'static str>" in gguf
    assert "fn read_model_info_stream(stream: &mut GgufStream) -> Result<GgufModelInfo, GgufError>" in gguf


def test_the_notice_names_the_file_and_counts_the_prompt():
    """An honest refusal quotes the file it read and says what went unanswered."""
    lib = lib_source()
    for phrase in (
        "Model file",
        " declares ",
        ", and states ",
        ", and states no ",
        " bytes were not answered.",
    ):
        assert phrase in lib, f"the service no longer says {phrase!r}"
    assert "model_gap_notice(" in generative_arm(lib)


def test_a_file_holding_no_tensors_is_not_called_an_unsupported_architecture():
    """A header that parses and names no tensor is a file with metadata and no
    weights yet -- which is exactly what a placeholder install leaves behind.

    WHY this is its own answer: the engine was never offered a tensor to reject,
    so "unsupported architecture" sends a reader looking for a model format
    problem when the device is holding an empty file. The notice quotes what the
    file declares and says the tensors are missing.
    """
    lib = lib_source()
    arm = generative_arm(lib)
    assert "ModelGap::NoTensors" in arm, "the zero-tensor branch no longer says what the file holds"
    assert "and holds no tensors" in lib, "the notice lost the ending that names an empty file"
    assert "ModelGap::Configured" in lib, "the two endings must stay distinguishable"


def test_the_notice_is_built_without_a_heap():
    """The service has no allocator, so the notice is written into a stack array."""
    lib = lib_source()
    assert "struct Notice<'a>" in lib
    assert "fn finish(self) -> &'a [u8]" in lib
    assert "let mut scratch = [0u8; NOTICE_WINDOW_BYTES];" in generative_arm(lib)
    assert "pub(crate) fn format_u32" in read("rag.rs")


def test_the_reply_that_names_a_gap_is_still_counted_in_bytes():
    """A reader can tell a whole answer from a cut one only by the length the
    header carries, and every answer counts bytes."""
    lib = lib_source()
    helper = function_body(lib, "fn emit_response_as")
    assert "output_dim: message.len() as u32" in helper
    # The generative reply keeps its own entry point, and it is the same writer.
    wrapper = function_body(lib, "fn emit_inference_response")
    assert "emit_response_as(" in wrapper
    assert "WHY the header counts bytes and not tokens" in doc_comment(
        lib, "fn emit_inference_response"
    )


def test_a_retrieval_frame_reaches_the_reader_whole_or_reports_short():
    """A search reply is a frame of records, and the mailbox holds 64 bytes.

    The service used to post the frame with a bare ipc_send, which the kernel
    cuts at the mailbox's limit without saying so; the reader then walked as
    many hits as the header counted over bytes that had never arrived. The
    frame now goes on the same channel the answers use, and only the inline
    route -- taken when no channel is free -- sends it by mailbox.
    """
    lib = lib_source()
    start = lib.index(GENERATIVE_END)
    search_arm = lib[start : start + lib[start:].index("// 4. Embedding inference query")]
    assert "stream_answer(" in search_arm
    assert "MSG_TYPE_SEARCH_CHUNK" in search_arm
    assert "&tx_buf[..cursor]" in search_arm
    # The two kinds of reply are told apart by the type their notice travels
    # on, so a frame cannot be read as an answer.
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs").read_text(
        encoding="utf-8"
    )
    assert "pub const MSG_TYPE_SEARCH_CHUNK: u32 = 0x5352_434B;" in sdk
    assert "MSG_TYPE_SEARCH_CHUNK, MSG_TYPE_SEARCH," not in sdk
    assert "pub const MSG_TYPE_SEARCH: u32 = 0x5352_4348;" in sdk


# The inline mailbox route: a 64-byte payload (`IPC_MAX_PAYLOAD`) with a 24-byte
# `AiInferenceResponse` header ahead of the text, so forty bytes of a notice reach
# the screen and the rest is cut with no word of it.
MAILBOX_NOTICE_ROOM = 40


def test_an_embedding_refusal_reaches_the_reader_whole_or_reports_short():
    """A loader refusal runs past a hundred bytes; the mailbox keeps forty of them.

    The service built a sentence naming the tensor, the byte counts and the bound,
    then posted it with `emit_response_as`, whose `ipc_send` the kernel cuts at
    `IPC_MAX_PAYLOAD` (64). After the 24-byte reply header that leaves forty bytes,
    so the Agent Manager painted half a refusal and the reader could not tell
    whether the model ran, the file was missing, or the read was refused. The whole
    notice now goes on the answer channel the other replies use -- the same route
    the search frame and the install report took -- and the mailbox is reached only
    when no channel is free, where the header's `output_dim` still names the true
    length so a cut is a reported shortfall rather than a silent one.
    """
    lib = lib_source()
    arm = without_comments(embedding_arm(lib))

    # A refusal is no longer posted through the mailbox, where the tail is dropped.
    assert (
        "emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf)" not in arm
    ), "an embedding refusal is still sent inline, where the mailbox keeps 40 bytes and cuts it"

    # The arm hands every refusal to the channel-route helper instead.
    assert "emit_embed_notice(" in arm, "the refusal lost its route through the answer channel"

    # The helper streams the notice whole and only then falls back to the mailbox.
    helper = function_body(lib, "fn emit_embed_notice")
    assert "stream_answer(" in helper, "the refusal helper no longer claims the answer channel"
    assert "MSG_TYPE_EMBED_CHUNK" in helper, "the refusal notice travels on no type of its own"
    assert (
        "emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf)" in helper
    ), "the refusal has no inline fallback for the case where no channel is free"

    # The bound that decides the cut is written down as arithmetic, not a magic word.
    assert (
        "const MAILBOX_NOTICE_ROOM: usize =" in lib
    ), "the 64-minus-24 room an inline notice has is no longer named where it is used"

    # The refusal chunk is a type of its own, distinct from the vector reply, so a
    # reader cannot stamp a sentence's byte count as an embedding dimension.
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs").read_text(
        encoding="utf-8"
    )
    assert "pub const MSG_TYPE_EMBED_CHUNK: u32 = 0x454D_434B;" in sdk
    assert "MSG_TYPE_EMBED_CHUNK, MSG_TYPE_EMBED," not in sdk
    assert "pub const MSG_TYPE_EMBED: u32 = 0x454D_4244;" in sdk


def test_the_embedding_transport_contract_can_fail():
    """A gate nobody has seen fail is a comment.

    Each block puts one fault back into a copy of the source and runs the same
    detector the gate above runs, so the shape the gate watches is proved to still
    be there and the check is proved to fire on it.
    """
    lib = lib_source()

    # 1. The silent cut restored: every refusal posted back through the mailbox.
    inline = lib.replace(
        "emit_embed_notice(sender_task_id, status, notice, tx_buf)",
        "emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf)",
    )
    assert inline != lib, "the refusal call sites moved; this fixture no longer matches them"
    cut_arm = without_comments(embedding_arm(inline))
    assert (
        "emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf)" in cut_arm
    ), "restoring the inline route did not put a mailbox-cuttable refusal back in the arm"
    assert "emit_embed_notice(" not in cut_arm, "the guard would not see the helper being bypassed"

    # 2. The channel route gutted: the helper answers only by mailbox.
    no_channel = lib.replace(
        "    if !stream_answer(sender_task_id, status, MSG_TYPE_EMBED_CHUNK, notice) {\n"
        "        emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf);\n"
        "    }",
        "    emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf);",
    )
    assert no_channel != lib, "the refusal helper's channel-first shape moved; fixture is stale"
    gutted = function_body(no_channel, "fn emit_embed_notice")
    assert "stream_answer(" not in gutted, "the guard would not see the answer channel dropped"
def test_an_install_moves_only_a_file_whose_header_parsed():
    """A model file is copied by the storage service, and only after its own
    header has been read.

    An install that copied first would put bytes in the model namespace that the
    platform then has to call weights without having read one of them.
    """
    body = function_body(lib_source(), "pub fn install_local_model")
    assert "gguf::find_installable_model" in body
    assert body.index("find_installable_model") < body.index("std::fs_write")
    finder = function_body(read("gguf.rs"), "pub fn find_installable_model")
    assert "read_gguf_header(path)" in finder, "the file is offered without a parsed header"


def test_the_install_request_carries_no_path():
    """Nobody outside the platform names which file an install moves.

    The request is four bytes, the source is one of two fixed trees, and the
    service that copies reads only the message type -- so an install cannot be
    pointed at a file the person never put in their own documents.
    """
    lib = lib_source()
    assert 'b"INSL"' in function_body(lib, "fn install_model_file")
    gguf = read("gguf.rs")
    assert (
        'pub const MODEL_SOURCE_TREES: &[&str] = &["/storage/downloads", "/data/incoming"];'
        in gguf
    )
    assert "for &tree in MODEL_SOURCE_TREES" in function_body(
        gguf, "pub fn find_installable_model"
    )
    intake = function_body(lib, "pub fn run_storage")
    assert "msg_type == std::ai::MSG_TYPE_INSTALL" in intake
    assert "install_local_model()" in intake


def test_the_copy_is_bounded_and_takes_no_heap():
    """The copy steps through the file, and a part copy is never a whole model."""
    body = function_body(lib_source(), "pub fn install_local_model")
    assert "let mut chunk = [0u8; INSTALL_CHUNK_BYTES];" in body
    assert "copied < stated_size" in body
    for heap in ("Vec::", "String::", "to_owned("):
        assert heap not in body, f"the install allocates with {heap!r}"


def test_the_install_is_answered_from_a_measurement():
    """The service says what the model namespace reports, because nothing else
    can tell it.

    The storage service holds the write and no send authority, so it cannot
    answer; a reason given here would be invented. The listing's byte count is
    the one statement available, and only a namespace holding exactly the bytes
    the source stated is called installed.
    """
    lib = lib_source()
    body = function_body(lib, "fn install_model_file")
    assert "gguf::local_model_size()" in body
    assert "std::ipc::receive" not in body, "the install answer relays a reply that cannot arrive"
    notice = function_body(lib, "fn install_notice")
    assert notice.count("STATUS_SUCCESS") == 1, "more than one outcome claims an install"
    assert "InstallOutcome::Installed(size)" in notice
    for refused in ("STATUS_UNAVAILABLE", "STATUS_ERROR"):
        assert refused in notice, f"the install answer never reports {refused}"


def test_an_install_refreshes_the_probe():
    """A probe taken once at startup keeps denying a file the service installed."""
    lib = lib_source()
    run_ai = function_body(lib, "pub fn run_ai")
    assert "let mut probed_models = gguf::probe_all_models();" in run_ai
    assert "MODEL_NAMESPACE_CHANGED.swap" in run_ai
    assert "MODEL_NAMESPACE_CHANGED.store(true" in function_body(
        lib, "fn install_model_file"
    )


def test_an_install_mints_no_new_authority():
    """The write stays with the one service the platform already grants it to.

    Widening the AI service's filesystem authority to serve an install would put
    a write door in front of everything the model is fed.
    """
    services = KERNEL_SERVICES.read_text(encoding="utf-8")
    inference = policy_arm(services, "Inference")
    assert "FileSystemWrite" not in inference, "the AI service was given the model namespace"
    assert "CapabilityType::IpcSend" in inference, "the install cannot be asked for"
    storage = policy_arm(services, "Storage")
    assert "CapabilityType::FileSystemWrite" in storage
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs").read_text(
        encoding="utf-8"
    )
    assert "pub const SERVICE_STORAGE_TASK_ID: u64 = 0xFFFF_0003;" in sdk
    assert "pub const MSG_TYPE_INSTALL: u32 = 0x494E_534C;" in sdk
    assert "pub const MSG_TYPE_INSTALL_CHUNK: u32 = 0x494E_5343;" in sdk


def _tokenizer_defects(gguf: str, lib: str) -> list:
    """Every way this ticket's walk can go wrong, as a list of what was seen.

    WHY a detector that returns its findings instead of asserting: the same run
    has to prove the live sources are clean and that a broken source is caught.
    A file of `assert`s can only do the first.
    """
    defects = []
    walk = function_body(gguf, "fn walk_tokenizer_kv")
    stepper = function_body(gguf, "fn step_over_string_array")
    bounded = function_body(gguf, "fn read_bounded_string")
    bool_reader = function_body(gguf, "fn read_bool")
    notice = function_body(lib, "fn tokenizer_clause")

    for heap in ("Vec::", "String::", "to_owned(", "copy_from_slice"):
        if heap in stepper:
            defects.append(f"the vocabulary step loads bytes into {heap!r}")
    if "stream.skip(len)" not in stepper:
        defects.append("the vocabulary is not passed by size arithmetic")
    if "metadata_field(" in walk:
        defects.append("a key was matched by its last dotted segment, not whole")
    if "if need > dst.len()" not in bounded:
        defects.append("a string was copied before its declared length was refused")
    if "if val_type != 7" not in bool_reader:
        defects.append("a bool is no longer read at the one byte GGUF type 7 is")
    if "$info.truncated_in = Some($key)" not in gguf:
        defects.append("a file that ends mid-value is no longer named by its key")
    if "info.truncated_in = Some(KEY_TOKENS)" not in walk:
        defects.append("a truncated vocabulary is reported as no vocabulary at all")
    if "VocabReading::Truncated {" not in walk:
        defects.append("a truncated vocabulary is reported as no vocabulary at all")
    if ", vocabulary counted not loaded" not in notice:
        defects.append("the notice calls a counted vocabulary a loaded one")
    if " bytes refused" not in notice:
        defects.append("the notice has lost the length a refused template stated")
    if "tokenizer absent, built-in template in use" not in notice:
        defects.append("a file with no tokenizer is reported as an empty template")
    if "let mut scratch = [0u8; NOTICE_WINDOW_BYTES];" not in generative_arm(lib):
        defects.append("the notice that carries the walk has no window of its own")
    if function_body(gguf, "pub fn missing_key").count("return Some(") != 7:
        defects.append("a tokenizer key was made mandatory, so a loadable file is now refused")
    return defects


def test_the_tokenizer_contract_can_fail():
    """A walk whose detectors cannot be tripped is a walk nobody has tested.

    Each source below is the live reader with one rule reversed: the vocabulary
    loaded instead of counted, a key matched by suffix, an over-long template
    cut to the window, a bool read at a float's width, a truncated file answered
    as a file with no vocabulary, a notice that calls a refused template a read
    one. The detector names every one, and stays silent on the tree as it is.
    """
    gguf = read("gguf.rs")
    lib = lib_source()
    assert _tokenizer_defects(gguf, lib) == []

    live_walk = function_body(gguf, "fn walk_tokenizer_kv")

    loading = gguf.replace(
        "        stream.skip(len)?;",
        "        let mut owned = host_std::vec::Vec::new();\n        stream.read_exact(&mut owned)?;",
    )
    assert _tokenizer_defects(loading, lib), "a walk that loaded the vocabulary passed"

    suffix = gguf.replace("let key = &key_buf[..key_len];", "let key = metadata_field(&key_buf[..key_len]);")
    assert _tokenizer_defects(suffix, lib), "a walk matching a key by suffix passed"

    clamped = gguf.replace(
        "    if need > dst.len() {\n        stream.skip(need)?;\n        return Ok(StringReading::RefusedTooLong(stated));\n    }\n    stream.read_exact(&mut dst[..need])?;",
        "    let taken = need.min(dst.len());\n    stream.read_exact(&mut dst[..taken])?;",
    )
    assert clamped != gguf, "the silent-cut fixture no longer matches the reader"
    assert _tokenizer_defects(clamped, lib), "a template cut to the window passed"

    wide_bool = gguf.replace("    if val_type != 7 {\n        return Err(GgufError::InvalidMetadata);\n    }\n    let mut byte = [0u8; 1];", "    let mut byte = [0u8; 4];")
    assert wide_bool != gguf, "the wide-flag fixture no longer matches the reader"
    assert _tokenizer_defects(wide_bool, lib), "a bool read at four bytes passed"

    lost_truncation = gguf.replace(live_walk, live_walk.replace("info.truncated_in = Some(KEY_TOKENS);\n", ""))
    assert lost_truncation != gguf, "the truncated-file fixture no longer matches"
    assert _tokenizer_defects(lost_truncation, lib), "a truncated file reported as clean passed"

    silent_notice = lib.replace(
        '        gguf::StringReading::RefusedTooLong(stated) => {\n            notice.word(b", template ");\n            notice.byte_count(stated);\n            notice.word(b" bytes refused");\n        }',
        '        gguf::StringReading::RefusedTooLong(_) => notice.word(b", template built-in default")',
    )
    assert silent_notice != lib, "the lying-notice fixture no longer matches"
    assert _tokenizer_defects(gguf, silent_notice), "a refused template called a default passed"

    demanding = gguf.replace(
        "pub fn missing_key(&self) -> Option<&'static str> {",
        "pub fn missing_key(&self) -> Option<&'static str> {\n"
        '        if self.template_reading == StringReading::Absent {\n'
        '            return Some("tokenizer.chat_template");\n'
        "        }",
    )
    assert demanding != gguf, "the mandatory-key fixture no longer matches"
    assert _tokenizer_defects(demanding, lib), (
        "a file the engine used to load is now refused, and nothing said so"
    )


def _segment(source: str, start: str, end: str) -> str:
    """The text from `start` up to the next `end` after it -- one match arm."""
    head = source.index(start)
    tail = source[head:]
    stop = tail.index(end) if end in tail else len(tail)
    return tail[:stop]


SDK = ROOT / "sdk" / "rust" / "interos-std" / "src" / "lib.rs"
KERNEL_SYSCALL = ROOT / "kernel" / "src" / "syscall" / "mod.rs"


def _ranged_read_defects(gguf: str, lib: str, sdk: str, kernel: str) -> list[str]:
    """What would make the file-backed reader pay for bytes it only steps over.

    A model file puts its tensor records after a vocabulary it will not load, so
    reaching the last tensor used to mean pushing every byte before it through a
    syscall and throwing it away. Reading at a named position is the whole of the
    fix, and a step is only free if the reader asks for nothing.
    """
    defects = []

    skip_body = function_body(gguf, "pub fn skip(&mut self, mut count: usize)")
    stepped = _segment(skip_body, "Self::File { pos, .. } => {", "Self::Buffered")
    if "std::" in stepped:
        defects.append("a step past a file still pays for the bytes it discards")

    read_body = function_body(gguf, "pub fn read_exact(&mut self, out: &mut [u8])")
    if "std::fs_read(" in read_body:
        defects.append("a file read moved the descriptor instead of naming a position")
    if "if n > remaining" not in read_body:
        defects.append("a count wider than the buffer it names is treated as bytes read")

    window_body = function_body(gguf, "fn refill_window(")
    if "if pulled > asked" not in window_body:
        defects.append("the window accepts more bytes than the space it asked for")
    if "std::fs_read(" in window_body:
        defects.append("the window reads forward instead of at its own position")

    # A step that costs nothing also never meets the end of the file, so the walk
    # that is about to speak about what a file declares asks for one byte first --
    # at both of the exits it can take.
    info_walk = function_body(gguf, "pub fn read_model_info_stream")
    if info_walk.count("source_ends_early(") != 2 or "stepped_past_the_end" not in gguf:
        defects.append("a walk that stepped past the end reports the file as if it read it")

    if "pub fn fs_read_at(" not in sdk:
        defects.append("the platform offers no positioned read to walk a file with")
    else:
        sdk_call = function_body(sdk, "pub fn fs_read_at(")
        if "SyscallNumber::FsReadAt" not in sdk_call:
            defects.append("the SDK's positioned read names a different call")

    if "FsReadAt = 164," not in kernel or "FsReadAt = 164," not in sdk:
        defects.append("the positioned read is not the same number on both sides")
    dispatch_marker = "164 => {"
    if dispatch_marker not in kernel:
        defects.append("nothing dispatches the positioned read")
    else:
        arm = function_body(kernel, dispatch_marker)
        if "sys_fs_read_at(" not in arm:
            defects.append("164 does not reach the positioned read")
        if not re.search(r"let\s+offset\s*=\s*a5\s+as\s+usize", arm):
            defects.append("the positioned read binds its offset from another register")
        if "MAX_FS_IO_BYTES" not in arm:
            defects.append("the positioned read is bounded by nothing")

    marker = "Err(gguf::GgufError::BufferTooShort"
    if marker not in lib:
        defects.append("a failed read no longer has an arm of its own")
    else:
        arm = _segment(lib, marker, "Err(_) =>")
        for variant in ("BufferTooShort", "ReadFailed", "FileNotFound"):
            if variant not in arm:
                defects.append(f"an unreadable file answers with {variant} unhandled")
        if "STATUS_UNAVAILABLE" not in arm:
            defects.append("a file that could not be read claims an unsupported architecture")
    return defects


def test_a_step_past_a_file_costs_no_request_and_a_read_names_its_position():
    """The walk reaches the last tensor without reading the bytes before it."""
    defects = _ranged_read_defects(
        read("gguf.rs"),
        lib_source(),
        SDK.read_text(encoding="utf-8"),
        KERNEL_SYSCALL.read_text(encoding="utf-8"),
    )
    assert defects == [], "; ".join(defects)


def test_the_ranged_read_contract_can_fail():
    """Each detector is tripped by the fault it exists for.

    The sources below are the live reader with one rule reversed: a step that
    reads the bytes it throws away, a read that moves the cursor, a count the
    reader accepts without bounding, a window that takes more than it asked for,
    a walk that never asks whether the file reaches where it stopped, the call
    taken out of the SDK, the number moved on one side of the boundary, the arm
    that answers it removed, its offset read from the wrong register, and a
    failed read answered as a corrupted model.
    """
    gguf = read("gguf.rs")
    lib = lib_source()
    sdk = SDK.read_text(encoding="utf-8")
    kernel = KERNEL_SYSCALL.read_text(encoding="utf-8")
    assert _ranged_read_defects(gguf, lib, sdk, kernel) == []

    stepping = gguf.replace(
        "                *pos = pos.checked_add(count).ok_or(GgufError::ArithmeticOverflow)?;\n                Ok(())",
        "                let mut scratch = [0u8; 512];\n"
        "                while count > 0 {\n"
        "                    let chunk = count.min(512);\n"
        "                    std::fs_read(*fd, &mut scratch[..chunk])?;\n"
        "                    count -= chunk;\n"
        "                }\n"
        "                Ok(())",
    )
    assert stepping != gguf, "the reading-step fixture no longer matches the reader"
    assert _ranged_read_defects(stepping, lib, sdk, kernel), "a step that paid for its bytes passed"

    cursor = gguf.replace("let n = std::fs_read_at(", "let n = std::fs_read(")
    assert cursor != gguf, "the cursor-moving fixture no longer matches the reader"
    assert _ranged_read_defects(cursor, lib, sdk, kernel), "a read that moved the descriptor passed"

    unbounded = gguf.replace(
        "                    if n > remaining {\n                        return Err(GgufError::ReadFailed);\n                    }\n",
        "",
    )
    assert unbounded != gguf, "the unbounded-count fixture no longer matches the reader"
    assert _ranged_read_defects(unbounded, lib, sdk, kernel), "an impossible byte count passed"

    greedy = gguf.replace(
        "                if pulled > asked {\n                    return Err(GgufError::ReadFailed);\n                }\n",
        "",
    )
    assert greedy != gguf, "the greedy-window fixture no longer matches the reader"
    assert _ranged_read_defects(greedy, lib, sdk, kernel), "a window overfilled past its buffer passed"

    unquestioning = gguf.replace(
        "        if info.is_complete() {\n            if source_ends_early(stream, header.tensor_count) {",
        "        if info.is_complete() {\n            if false {",
    )
    assert unquestioning != gguf, "the unquestioning-walk fixture no longer matches the reader"
    assert _ranged_read_defects(unquestioning, lib, sdk, kernel), (
        "a walk that stepped past a truncated file and said nothing passed"
    )

    no_position = sdk.replace("pub fn fs_read_at(", "pub fn fs_read_at_removed(")
    assert no_position != sdk, "the missing-call fixture no longer matches the SDK"
    assert _ranged_read_defects(gguf, lib, no_position, kernel), "a walk with no positioned read passed"

    renamed = sdk.replace("FsReadAt = 164,", "FsReadAt = 165,")
    assert renamed != sdk, "the moved-number fixture no longer matches the SDK"
    assert _ranged_read_defects(gguf, lib, renamed, kernel), (
        "the two sides naming different numbers for one call passed"
    )

    undispatched = kernel.replace("164 => {", "164_removed => {")
    assert undispatched != kernel, "the removed-arm fixture no longer matches the kernel"
    assert _ranged_read_defects(gguf, lib, sdk, undispatched), (
        "a call number no handler answers passed"
    )

    wrong_register = kernel.replace("let offset = a5 as usize;", "let offset = a4 as usize;")
    assert wrong_register != kernel, "the wrong-register fixture no longer matches the kernel"
    assert _ranged_read_defects(gguf, lib, sdk, wrong_register), (
        "an offset read from the length register passed"
    )

    blamed = lib.replace(
        "Err(gguf::GgufError::BufferTooShort | gguf::GgufError::ReadFailed | gguf::GgufError::FileNotFound) => {",
        "Err(gguf::GgufError::BufferTooShort | gguf::GgufError::FileNotFound) => {",
    )
    assert blamed != lib, "the misreported-fault fixture no longer matches the service"
    assert _ranged_read_defects(gguf, blamed, sdk, kernel), "an unreadable file called corrupt passed"


def _call_args(source: str, callee: str) -> list[str]:
    """The argument text of every call to `callee`, with comments already dropped."""
    calls = []
    needle = callee + "("
    cursor = 0
    while True:
        index = source.find(needle, cursor)
        if index < 0:
            return calls
        open_at = index + len(needle) - 1
        depth = 0
        for position in range(open_at, len(source)):
            char = source[position]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    calls.append(source[open_at + 1 : position])
                    break
        cursor = index + len(needle)


def _tensor_position_defects(gguf: str, readers: dict) -> list[str]:
    """What would put a tensor's bytes at the wrong position in the file.

    A GGUF descriptor's `offset` counts from the start of the tensor data section,
    which begins after the descriptor table plus whatever padding the file's own
    alignment asks for. The published EmbeddingGemma file puts that start 6,530,464
    bytes into the file, so a reader that hands the descriptor's number straight to
    a positioned read asks for the vocabulary instead of the weights -- and asks
    successfully, because the bytes it names really exist.
    """
    defects = []

    locate = function_body(gguf, "pub fn locate_tensor_stream(")
    walk = locate.index("for _ in 0..header.tensor_count")
    placement = locate.index("align_up(stream.position()") if "align_up(stream.position()" in locate else -1
    if placement < 0:
        defects.append("the data section does not start where the tensor table ended")
    elif placement < walk:
        defects.append("the data section is placed before the tensor table has been passed")
    if "checked_add(tensor.offset)" not in locate:
        defects.append("a tensor's file position is the offset the file states, unplaced")
    if "is_power_of_two()" not in locate:
        defects.append("an alignment the format cannot place data with is rounded instead")
    if "InvalidAlignment" not in locate:
        defects.append("a corrupt alignment is used to place the tensors anyway")
    if "MAX_TENSOR_ALIGNMENT" not in locate:
        defects.append("a declared alignment is bounded by nothing")

    alignment_walk = function_body(gguf, "fn read_alignment_stream(")
    if "let mut alignment = DEFAULT_ALIGNMENT;" not in alignment_walk:
        defects.append("a file that names no alignment is treated as having an unknown layout")
    if 'b"general.alignment"' not in alignment_walk:
        defects.append("the key that says where the data starts is never read")

    # `offset` means two different things in this file, and only one of them can be
    # given to a read. A caller that reaches for the wrong one is not caught by the
    # compiler, so it is caught here.
    for name, source in readers.items():
        for args in _call_args(without_comments(source), "std::fs_read_at"):
            if ".offset" in args and "file_position" not in args:
                defects.append(f"{name} gives a positioned read the offset a file states")
    return defects


def test_a_tensor_read_is_given_where_the_data_section_starts():
    """A tensor's offset is resolved to a file position before anything reads it."""
    readers = {
        path.name: path.read_text(encoding="utf-8")
        for path in sorted(SERVICE.rglob("*.rs"))
        if path.name != "gguf.rs"
    }
    defects = _tensor_position_defects(read("gguf.rs"), readers)
    assert defects == [], "; ".join(defects)


def test_the_tensor_position_contract_can_fail():
    """Each detector is tripped by the fault it exists for.

    The sources below are the live resolver with one rule reversed: the data section
    counted from the header instead of the end of the table, the table placed before
    it was walked, the offset handed over as the whole position, an odd alignment
    rounded up, a corrupt alignment accepted, an unbounded one, a default of zero
    where the format means thirty-two, the alignment key never read, and a caller
    that gives a positioned read the number relative to the section.
    """
    gguf = read("gguf.rs")
    readers = {
        path.name: path.read_text(encoding="utf-8")
        for path in sorted(SERVICE.rglob("*.rs"))
        if path.name != "gguf.rs"
    }
    assert _tensor_position_defects(gguf, readers) == []

    from_header = gguf.replace(
        "let data_start = align_up(stream.position() as u64, alignment)?;",
        "let data_start = align_up(GgufHeader::HEADER_SIZE as u64, alignment)?;",
    )
    assert from_header != gguf, "the fixture no longer matches the resolver"
    assert _tensor_position_defects(from_header, readers), (
        "a data section counted from the header was accepted"
    )

    early = gguf.replace(
        "    let mut found: Option<GgufTensorInfo> = None;",
        "    let data_start = align_up(stream.position() as u64, alignment)?;\n"
        "    let mut found: Option<GgufTensorInfo> = None;",
    )
    assert early != gguf, "the ordering fixture no longer matches the resolver"
    assert _tensor_position_defects(early, readers), (
        "a data section placed before the tensor table was walked was accepted"
    )

    unplaced = gguf.replace(
        "    let file_position = data_start\n        .checked_add(tensor.offset)\n"
        "        .ok_or(GgufError::ArithmeticOverflow)?;",
        "    let file_position = tensor.offset;",
    )
    assert unplaced != gguf, "the unplaced-offset fixture no longer matches the resolver"
    assert _tensor_position_defects(unplaced, readers), (
        "an offset relative to the data section was accepted as a file position"
    )

    rounded = gguf.replace(
        "if !alignment.is_power_of_two() || alignment > MAX_TENSOR_ALIGNMENT {",
        "if alignment == 0 {",
    )
    assert rounded != gguf, "the rounding fixture no longer matches the resolver"
    assert _tensor_position_defects(rounded, readers), "a non-power-of-two alignment was placed with"

    used_anyway = gguf.replace(
        "return Err(GgufError::InvalidAlignment(alignment));",
        "let alignment = DEFAULT_ALIGNMENT;",
    )
    assert used_anyway != gguf, "the refusal fixture no longer matches the resolver"
    assert _tensor_position_defects(used_anyway, readers), "a corrupt alignment went unrefused"

    unbounded = gguf.replace(
        "if !alignment.is_power_of_two() || alignment > MAX_TENSOR_ALIGNMENT {",
        "if !alignment.is_power_of_two() {",
    )
    assert unbounded != gguf, "the bound fixture no longer matches the resolver"
    assert _tensor_position_defects(unbounded, readers), "a huge declared alignment was honoured"

    no_default = gguf.replace(
        "let mut alignment = DEFAULT_ALIGNMENT;",
        "let mut alignment = 0;",
    )
    assert no_default != gguf, "the default fixture no longer matches the resolver"
    assert _tensor_position_defects(no_default, readers), (
        "a file that names no alignment was called unplaceable"
    )

    key_unread = gguf.replace('b"general.alignment"', 'b"general.file_type"')
    assert key_unread != gguf, "the key fixture no longer matches the resolver"
    assert _tensor_position_defects(key_unread, readers), (
        "a file that declares an alignment was placed at the default"
    )

    lying_caller = {
        "loader.rs": (
            "fn pull(weights: &TensorLocation, scratch: &mut [u8], fd: u32) {\n"
            "    let _ = std::fs_read_at(fd, weights.tensor.offset, scratch);\n"
            "}\n"
        )
    }
    assert _tensor_position_defects(gguf, lying_caller), (
        "a read handed the section-relative offset passed the contract"
    )
    # The same call with the resolved position is what the loader is meant to write.
    honest_caller = {
        "loader.rs": (
            "fn pull(weights: &TensorLocation, scratch: &mut [u8], fd: u32) {\n"
            "    let _ = std::fs_read_at(fd, weights.file_position, scratch);\n"
            "}\n"
        )
    }
    assert _tensor_position_defects(gguf, honest_caller) == []


# The loader's own rules, checked over `weights.rs`.
#
# WHY a detector set for a module that did not exist when this file was written:
# the whole reason the service could price a model and answer nothing from it was
# that no code turned the installed file's tensors into the graph's shapes, and the
# two ways to close that hop badly are both invisible to the compiler -- holding the
# file (a 213,909,504-byte embedding table does not fit a run that is allowed
# 125,829,120 bytes) and answering a dtype the engine has no layout for with a
# neighbour's arithmetic. A green unit test proves neither of those away; these
# strings do.
def _loader_defects(weights: str) -> list[str]:
    code = without_comments(weights)
    defects = []

    if "read_to_end" in code:
        defects.append("the loader pulls a whole tensor into one buffer")
    if "fs_read(" in code.replace("fs_read_at(", ""):
        defects.append("a tensor is read through a cursor the loader would have to move")

    seam = function_body(weights, "impl PositionRead for ModelFileDescriptor")
    if "fs_read_at(self.fd, position" not in seam:
        defects.append("the device's read is not positioned where the descriptor says")
    if "got == 0" not in seam:
        defects.append("a read that returns nothing is retried forever instead of refused")

    block = function_body(weights, "pub fn load_block(&mut self, index: usize)")
    if "self.slot.release()" not in block:
        defects.append("a block's tensors stay held while the next block is read")
    if "quantised_row_bytes(handle.dtype" not in block:
        defects.append("a block is dequantised with a type its descriptors did not declare")
    if "UnsupportedDtype" not in block:
        defects.append("a dtype the engine has no layout for is read as another one")
    if "MixedBlockDtype" not in block:
        defects.append("a block split across two quantisations is answered with one of them")
    if "weight_type" not in code:
        defects.append("the dtype the block declared never reaches the graph")

    row = function_body(weights, "pub fn load_embedding_row(")
    if "quantised_row_bytes(entry.dtype" not in row:
        defects.append("an embedding row is cut to a width the file's dtype does not give")
    if ">= entry.rows" not in row:
        defects.append("a token id past the table wraps instead of being refused")

    run = function_body(weights, "pub fn run_streamed_embedding<")
    if "config.check_scratch_budget()" not in run:
        defects.append("the streamed run skips the budget every other run is priced by")
    if "reader.load_block(block)" not in run:
        defects.append("the streamed run does not read the block it runs")
    if "embedding_row_into(" not in run:
        defects.append("the row read for a token is never dequantised into the stream")
    if "kv_cache.len = pos + 1" not in run:
        defects.append("a position's key/value writes are left visible to the next one")
    return defects


def test_the_loader_reads_the_installed_file_one_block_at_a_time():
    """The hop from a descriptor to a forward pass, bounded and dtype-honest."""
    weights = read("weights.rs")
    defects = _loader_defects(weights)
    assert defects == [], "; ".join(defects)
    # The slot is emptied between blocks, so 24 never coexist: the release is the
    # first statement of the load, not a courtesy call after the reads.
    block = function_body(weights, "pub fn load_block(&mut self, index: usize)")
    assert block.index("self.slot.release()") < block.index(
        "for suffix in BLOCK_NORMS"
    ), "the block refills its slot before emptying it"


def test_the_loader_contract_can_fail():
    """Each loader detector is tripped by the fault it exists for.

    The sources below are the live loader with one rule reversed: the slot left
    standing while the next block is read, a tensor pulled to the end of the file
    instead of read at its position, a block dequantised with a type no descriptor
    declared, a token id past the embedding table read as a row of the file's, and a
    streamed run that never prices itself against the byte budget.
    """
    weights = read("weights.rs")
    assert _loader_defects(weights) == []

    held = weights.replace("        self.slot.release();\n", "", 1)
    assert held != weights, "the release is no longer where the load starts"
    assert any("next block is read" in defect for defect in _loader_defects(held)), (
        "a loader that keeps 24 blocks alive passed the contract"
    )

    slurped = weights.replace(
        "        self.source.read_at(entry.position, &mut buffer)?;",
        "        self.source.read_at(entry.position, &mut buffer)?;\n"
        "        let _ = file.read_to_end(&mut buffer);",
        1,
    )
    assert slurped != weights, "the payload read the whole-tensor rule watches is gone"
    assert any("whole tensor" in defect for defect in _loader_defects(slurped)), (
        "a loader that read a tensor to the end of the file passed the contract"
    )

    guessed_dtype = weights.replace(
        "            if quantised_row_bytes(handle.dtype, handle.cols as usize).is_none() {",
        "            if false {",
        1,
    )
    assert guessed_dtype != weights, "the dtype check the contract watches is gone"
    assert any(
        "descriptors did not declare" in defect for defect in _loader_defects(guessed_dtype)
    ), "a block dequantised with an assumed type passed the contract"

    no_range = weights.replace(
        "        if (token as u64) >= entry.rows {", "        if false {", 1
    )
    assert no_range != weights, "the row-range refusal the contract watches is gone"
    assert any("wraps instead" in defect for defect in _loader_defects(no_range)), (
        "a token id past the table was read as a row of the file's"
    )

    unbudgeted = weights.replace("    config.check_scratch_budget()?", "    let _ = 0;", 1)
    assert unbudgeted != weights, "the budget call the run is priced by is gone"
    assert any("budget every other run" in defect for defect in _loader_defects(unbudgeted)), (
        "a streamed run that skipped the byte budget passed the contract"
    )


def _byte_literal(text: str) -> int:
    """`64 * 1024` as the number it writes.

    WHY not `eval`: the two sides of the syscall declare the same bound in their
    own file, and the contract is that the numbers agree. Multiplying the factors
    is the whole of what reading a `usize` literal needs.
    """
    value = 1
    for token in text.split("*"):
        value *= int(token.strip().replace("_", ""))
    return value


# The platform's per-request read bound, checked on both sides of the syscall.
#
# WHY this exists at all: the kernel answers one positioned read of at most
# `MAX_FS_IO_BYTES` and refuses anything wider with `InvalidArguments`, while the
# published file's tensors are megabytes -- `blk.0.attn_q.weight` is 2,507,520
# bytes. On a real on-guest probe the loader handed the call the whole tensor and
# the refusal came back as `ShortRead`, so a 64 KiB ABI bound reached the screen as
# "an install that stopped part-way" for a 333,590,944-byte file that is whole. The
# bound is correct and the request was not; these detectors keep the split at the
# descriptor seam and keep the two failures from collapsing back into one.
def _bounded_read_defects(weights: str, kernel: str, lib: str) -> list[str]:
    """What would let one over-wide read be reported as a truncated model."""
    defects = []

    kernel_bound = re.search(r"const MAX_FS_IO_BYTES: usize = ([0-9_ *]+);", kernel)
    caller_bound = re.search(
        r"pub const MAX_POSITIONED_READ_BYTES: usize = ([0-9_ *]+);", weights
    )
    if kernel_bound is None or caller_bound is None:
        defects.append("the per-request read bound is not stated on both sides")
    elif _byte_literal(kernel_bound.group(1)) != _byte_literal(caller_bound.group(1)):
        defects.append("the caller's read bound is not the kernel's")

    arm = function_body(kernel, "164 => {")
    if "buf_len > MAX_FS_IO_BYTES" not in arm:
        defects.append("the kernel no longer refuses a request wider than its bound")

    seam = function_body(weights, "impl PositionRead for ModelFileDescriptor")
    if "read_in_bounded_chunks(" not in seam:
        defects.append("the descriptor hands the platform the caller's whole slice")
    if "ChunkRead::End" not in seam or "ChunkRead::Refused" not in seam:
        defects.append("the descriptor reports one failure where the platform gives two")

    loop = function_body(weights, "pub fn read_in_bounded_chunks<")
    if "MAX_POSITIONED_READ_BYTES" not in loop:
        defects.append("the chunking loop is bounded by nothing")
    if "&mut into[filled..filled + wanted]" not in loop:
        defects.append("the chunking loop asks for the whole rest of the request")
    if (
        "ChunkRead::Got(0) | ChunkRead::End =>" not in loop
        or "LoadError::ShortRead(chunk_position, asked, filled)" not in loop
    ):
        defects.append("a file that ends mid-request stopped being a short read")
    if "LoadError::ReadRefused(chunk_position, wanted, errno)" not in loop:
        defects.append("a platform refusal is reported as a short file again")
    if "LoadError::OverDeliveredChunk(chunk_position, wanted, got)" not in loop:
        defects.append("a count wider than its window is walked past instead of refused")

    notice = function_body(lib, "fn file_embed_notice<'a>(")
    if "weights::LoadError::ReadRefused" not in notice:
        defects.append("the platform refusal has no arm of its own in the notice")
    if "weights::LoadError::OverDeliveredChunk" not in notice:
        defects.append("the over-wide count has no arm of its own in the notice")
    if "an install that stopped part-way" not in notice:
        defects.append("the short-file sentence stopped naming the fact it reports")
    return defects


def test_a_tensor_wider_than_the_platform_read_bound_is_read_in_chunks():
    """The split happens at the descriptor, and the two refusals stay separate."""
    defects = _bounded_read_defects(
        read("weights.rs"),
        KERNEL_SYSCALL.read_text(encoding="utf-8"),
        lib_source(),
    )
    assert defects == [], "; ".join(defects)


def test_the_bounded_read_contract_can_fail():
    """Each detector above is tripped by the fault it exists for.

    The sources below are the live loader with one rule reversed: the bound raised
    on one side of the syscall only, the kernel's refusal removed, the descriptor
    back to one call per tensor, the window left unbounded, a refusal collapsed into
    a short read, an over-wide count believed, and the notice arm deleted.
    """
    weights = read("weights.rs")
    kernel = KERNEL_SYSCALL.read_text(encoding="utf-8")
    lib = lib_source()
    assert _bounded_read_defects(weights, kernel, lib) == []

    raised = weights.replace(
        "pub const MAX_POSITIONED_READ_BYTES: usize = 64 * 1024;",
        "pub const MAX_POSITIONED_READ_BYTES: usize = 4 * 1024 * 1024;",
        1,
    )
    assert raised != weights, "the caller's bound the contract watches is gone"
    assert any("not the kernel's" in d for d in _bounded_read_defects(raised, kernel, lib)), (
        "a loader that over-asks by sixty-four still passed"
    )

    unbounded = kernel.replace(
        "const MAX_FS_IO_BYTES: usize = 64 * 1024;",
        "const MAX_FS_IO_BYTES: usize = 4 * 1024 * 1024;",
        1,
    )
    assert unbounded != kernel, "the kernel's bound the contract watches is gone"
    assert any("not the kernel's" in d for d in _bounded_read_defects(weights, unbounded, lib)), (
        "the two sides disagreeing went unnoticed"
    )

    no_refusal = kernel.replace(
        "            if buf_len > MAX_FS_IO_BYTES {\n"
        "                return SyscallResult::InvalidArguments as i64;\n"
        "            }\n"
        "            let mut kbuf = [0u8; 4096];\n"
        "            let mut done = 0usize;\n"
        "            while done < buf_len {\n"
        "                let want = (buf_len - done).min(kbuf.len());\n"
        "                match sys_fs_read_at(cap_slot, fd, offset + done, &mut kbuf[..want]) {",
        "            let mut kbuf = [0u8; 4096];\n"
        "            let mut done = 0usize;\n"
        "            while done < buf_len {\n"
        "                let want = (buf_len - done).min(kbuf.len());\n"
        "                match sys_fs_read_at(cap_slot, fd, offset + done, &mut kbuf[..want]) {",
        1,
    )
    assert no_refusal != kernel, "the refusal the kernel rule watches is gone"
    assert any(
        "no longer refuses" in d for d in _bounded_read_defects(weights, no_refusal, lib)
    ), "an unbounded positioned read passed the contract"

    single_shot = weights.replace(
        "        read_in_bounded_chunks(position, into, |position, window| {",
        "        match crate::std::fs_read_at(self.fd, position, into) {",
        1,
    )
    assert single_shot != weights, "the seam the chunking rule watches moved"
    assert any(
        "caller's whole slice" in d
        for d in _bounded_read_defects(single_shot, kernel, lib)
    ), "the descriptor went back to one call per tensor and nothing said so"

    whole_window = weights.replace(
        "match one_read(chunk_position, &mut into[filled..filled + wanted]) {",
        "match one_read(chunk_position, &mut into[filled..]) {",
        1,
    )
    assert whole_window != weights, "the window the loop is watched on moved"
    assert any(
        "whole rest of the request" in d
        for d in _bounded_read_defects(whole_window, kernel, lib)
    ), "a loop that asks for the whole remainder passed the contract"

    collapsed = weights.replace(
        "return Err(LoadError::ReadRefused(chunk_position, wanted, errno));",
        "return Err(LoadError::ShortRead(chunk_position, asked, filled));",
        1,
    )
    assert collapsed != weights, "the refusal arm the contract watches moved"
    assert any(
        "short file again" in d for d in _bounded_read_defects(collapsed, kernel, lib)
    ), "a platform refusal reported as a truncated install passed the contract"

    believed = weights.replace(
        "return Err(LoadError::OverDeliveredChunk(chunk_position, wanted, got));",
        "filled += got;",
        1,
    )
    assert believed != weights, "the count guard the contract watches moved"
    assert any(
        "walked past instead of refused" in d
        for d in _bounded_read_defects(believed, kernel, lib)
    ), "a source that lies about its own count was believed"

    merged_notice = lib.replace(
        "FileEmbedError::Load(weights::LoadError::ReadRefused(position, asked, errno)) => {",
        "FileEmbedError::Load(weights::LoadError::Arithmetic) => {",
        1,
    )
    assert merged_notice != lib, "the notice arm the contract watches moved"
    assert any(
        "no arm of its own in the notice" in d
        for d in _bounded_read_defects(weights, kernel, merged_notice)
    ), "the refusal lost its sentence and the contract did not notice"


def test_the_loader_runs_the_installed_file_or_says_it_did_not():
    """The one case that runs the graph over the 333 MB file has to say when it did not.

    Every other case prices a hand-written description of the file, so a loader wrong
    about the real bytes still passes them. The gate is the same one the engine's own
    real-file case uses, and it has to carry the numbers a reader can check: bytes
    read, the peak the run held, the dot product of two different prompts' vectors.
    """
    weights = read("weights.rs")
    body = function_body(
        weights, "fn test_the_installed_file_is_read_block_by_block_and_run"
    )
    assert "INTEROS_REAL_GGUF_PATH" in body, "the real file is no longer opt-in, or is silent"
    assert "NOT VERIFIED" in body, (
        "a run with no file in front of it would pass without saying it read nothing"
    )
    for shipping in (
        "walk_the_installed_table",
        "locate_tensor",
        "ModelConfig::from_gguf",
        "load_block",
        "load_embedding_row",
        "embedding_row_into",
        "run_streamed_embedding",
        "Scratch::new",
        "KvCache::new",
    ):
        assert shipping in body, f"the real-file case no longer runs {shipping!r}"
    # The prefix walker it hands the path to is the shipping walk, not a second
    # parser written for the test.
    assert "TensorTable::walk(&mut stream)" in function_body(
        weights, "pub(crate) fn walk_the_installed_table"
    ), "the real-file case resolves its table with something other than the loader's walk"
    for measurement in ("bytes read", "peak slot", "dot("):
        assert measurement in body, f"the run no longer reports its {measurement!r}"
    assert "HOST-BACKED" in body, (
        "a host running the engine is about to be read as the device doing it"
    )


def test_the_notice_narrows_its_refusal_instead_of_losing_it():
    """The screen may say what a reply skipped, not what the engine cannot do.

    WHY this is a contract and not a comment: the clause used to read "whose tensors
    the engine does not read", which was true only while no loader existed. Once the
    loader reads an installed file's tensors, keeping that sentence is a false claim
    about capability and deleting it is a gate passing because the lie was removed --
    so the narrowed wording is pinned here, and the gated case that licenses it (the
    same file handed to both the notice and the loader) has to stay in `lib.rs`.
    """
    lib = without_comments(read("lib.rs"))
    assert "whose tensors the engine does not read" not in lib, (
        "the notice claims the engine reads no tensor of the file, which the loader's "
        "measurement against the installed file contradicts"
    )
    assert "none of whose tensors this reply reads" in lib, (
        "the refusal stopped saying what this reply did not read"
    )
    assert ", and holds no tensors" in lib, (
        "an empty file and a file whose tensors went unread are now one sentence"
    )
    gated = function_body(
        read("lib.rs"), "fn test_the_notice_for_the_file_this_host_holds_quotes_that_file"
    )
    assert "walk_the_installed_table" in gated and "load_block" in gated, (
        "the sentence about the real file is no longer checked against the loader "
        "reading that same file"
    )
    assert "NOT VERIFIED" in gated, "the real-file notice case can pass having opened nothing"


def test_the_contract_can_fail():
    """A contract that cannot fail is a comment.

    The detectors run here over an arm carrying the faults they exist to catch;
    the same detectors then run over the service as it stands and stay silent.
    """
    synthetic = "\n".join(
        [
            GENERATIVE_START,
            "let config = inference::ModelConfig::default_test_config();",
            "let id_w = [0u8; 64 * 64 * 4];",
            "let weights = ModelWeights { token_embd: &id_w, ..LayerWeights::empty() };",
            "let _ = inference::generate(&tokens, &weights, &config, 8, 0.0, &cancel, |t| true);",
            "reply_with_answer(sender_task_id, std::ai::STATUS_SUCCESS, answer, tx_buf);",
            GENERATIVE_END,
        ]
    )
    arm = generative_arm(synthetic)
    assert "std::ai::STATUS_SUCCESS" in arm
    for fabricated in ("inference::generate(", "ModelWeights", "default_test_config("):
        assert fabricated in without_comments(arm), f"the arm under test no longer builds {fabricated!r}"

    live_arm = generative_arm(lib_source())
    assert "std::ai::STATUS_SUCCESS" not in live_arm
    assert "default_test_config" not in live_arm
    assert "inference::generate(" not in without_comments(live_arm)

    # A file whose header parses and holds no tensor, answered as an architecture
    # problem, is what the empty-file detector exists to catch.
    assert live_arm.count("ModelGap::NoTensors") == 1
    blamed_on_the_architecture = live_arm.replace("ModelGap::NoTensors", "ModelGap::Configured")
    assert "ModelGap::NoTensors" not in blamed_on_the_architecture, \
        "the empty-file detector has nothing left to trip on"

    # An install that names a part copy installed, takes its source from the
    # wire, or is served by a service holding the write, each trips its detector.
    lying_install = "\n".join(
        [
            "fn install_notice(outcome: InstallOutcome, out: &mut [u8]) -> (u32, &[u8]) {",
            "match &outcome {",
            "InstallOutcome::Installed(size) => { std::ai::STATUS_SUCCESS }",
            "InstallOutcome::Short(_, _) => { std::ai::STATUS_SUCCESS }",
            "InstallOutcome::Absent(_) => { std::ai::STATUS_ERROR }",
            "}",
            "}",
        ]
    )
    notice = function_body(lying_install, "fn install_notice")
    assert notice.count("STATUS_SUCCESS") == 2, "the counter no longer sees two claims"
    assert "STATUS_UNAVAILABLE" not in notice, "the refusal detector has lost its target"

    path_from_wire = "\n".join(
        [
            "pub fn install_local_model(payload: &[u8]) -> Option<u64> {",
            "let path = core::str::from_utf8(&payload[..]).ok()?;",
            "let fd = std::fs_open(path, std::FS_WRITE).ok()?;",
            "let _ = std::fs_write(fd, payload);",
            "}",
        ]
    )
    body = function_body(path_from_wire, "pub fn install_local_model")
    assert "gguf::find_installable_model" not in body
    assert "copied < stated_size" not in body

    widened = "\n".join(
        [
            "ServiceId::Inference => {",
            "policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemWrite, rights: Rights::WRITE, object_id: 0 });",
            "}",
        ]
    )
    assert "FileSystemWrite" in policy_arm(widened, "Inference")
    assert "FileSystemWrite" not in policy_arm(
        KERNEL_SERVICES.read_text(encoding="utf-8"), "Inference"
    )


# The embedding arm's honesty, checked over `lib.rs` and `ai.rs`.
#
# WHY this section exists at all: every gate above was written against the
# generative arm, and the embedding arm -- the one that answers a vector -- was
# ungated. It computed a hash projection of the prompt's bytes, sent it with
# `status: 0`, and `STATUS_SUCCESS` is the code a reader is told means "from the
# weights". Meanwhile the search arm stamped `is_neural` from a directory
# listing, so the first person to install a file would have had every lexical
# match relabelled as a neural one. A change that fixes both is invisible to
# every assertion above it, so the rule each fix replaced is pinned here.
# WHY a code marker and not the `// 4. Embedding...` comment the other arms use:
# several gates below slice a comment-stripped arm, where a comment marker has
# already been removed -- and a helper that raises on the source it is handed
# gets replaced by one that quietly returns the whole file.
EMBEDDING_START = "let mut target_dim = embedding::EMBEDDING_DIM;"


def embedding_arm(source: str) -> str:
    """The dispatch arm that answers an embedding query, and nothing else."""
    dispatch = function_body(source, "pub fn handle_ipc_inference_query(")
    return dispatch[dispatch.index(EMBEDDING_START) :]


SEARCH_START = "let mut target_dim = 256usize;"


def search_arm(source: str) -> str:
    """The retrieval arm, and nothing beside it.

    WHY it stops where the embedding arm starts: `is_neural` is a field only this
    arm writes, so a gate that read the rest of the dispatch function could pass
    on a label the embedding arm invented.
    """
    dispatch = function_body(source, "pub fn handle_ipc_inference_query(")
    return dispatch[dispatch.index(SEARCH_START) : dispatch.index(EMBEDDING_START)]


def test_the_embedding_arm_labels_its_lexical_baseline():
    """A hash of the prompt may never again be reported as a reply from weights.

    The baseline is a legitimate answer -- it is deterministic, it costs nothing,
    and it ranks documents today. What it never was is a model's embedding. The
    status is where a reader learns which one it got, so the arm must emit the
    baseline status on the baseline branch and must not reach `STATUS_SUCCESS`
    from anywhere that has not read a tensor.
    """
    lib = lib_source()
    baseline = function_body(lib, "fn reply_lexical_baseline(")
    assert "embedding::compute_embedding_768(" in baseline, (
        "the baseline stopped being the lexical projection it is named for"
    )
    assert "status: STATUS_BASELINE_LEXICAL" in baseline, (
        "the baseline reply no longer carries the status that says it is a baseline"
    )
    assert "STATUS_SUCCESS" not in baseline, "the baseline claims weights again"

    # The arm's code with comments dropped: a comment may name the success it is
    # refusing to send, and one that names it while sending the baseline is the
    # fault. `STATUS_SUCCESS` is allowed only past the point where the
    # file-backed branch has had a run reported `Ok` -- which is the split below.
    arm = without_comments(embedding_arm(lib))
    assert "EmbeddingPlan::RunInstalledFile =>" in arm
    before, after = arm.split("EmbeddingPlan::RunInstalledFile =>", 1)
    assert "STATUS_SUCCESS" not in before, (
        "the embedding arm picks a success status outside the file-backed branch"
    )
    assert "STATUS_SUCCESS" in after, (
        "a run that read the file's tensors no longer answers with the neural status"
    )
    # One place computes the projection, and it is the labelled one.
    assert "compute_embedding_768" not in arm, (
        "a second projection appeared in the arm, beside the labelled one"
    )
    assert "EmbeddingPlan::LexicalBaseline =>" in arm
    assert "reply_lexical_baseline(" in arm


def test_a_file_on_the_device_earns_no_neural_claim_by_existing():
    """`is_neural` and the neural status answer "what ran", not "what is installed".

    This is the whole defect the search arm carried: `is_neural =
    probed_models.embedding.is_some()` was true the instant a file landed, while
    `rag::RagIndex` went on embedding both the chunks and the query with the same
    hash projection it had always used. The gate is a shape check, because the
    wrong side to be on is the one where a green suite means a false label.
    """
    lib = without_comments(lib_source())
    arm = embedding_arm(lib)

    # Search: the flag comes off the index that scored the match.
    retrieval = search_arm(lib)
    assert "is_neural: m.is_neural as u8" in retrieval, (
        "the search hit no longer relays the index's own flag"
    )
    assert "rag::RagIndex::new(false)" in retrieval, (
        "the retrieval index was built under a neural flag it does not earn; what "
        "it ingests is the lexical projection, so nothing it matches is neural"
    )
    for site in ("is_neural = probed_models.embedding.is_some()",
                 "if probed_models.embedding.is_some() { 1 } else { 0 }"):
        assert site not in lib, f"the arm derives a neural label from the probe at {site!r}"

    # Embedding: the probe is an input to a decision, never the decision itself.
    assert (
        "plan_embedding_run( probed_models.embedding.is_some(), token_ids.map(|ids| ids.len), strict_neural_required, )"
        in flat(arm)
    ), (
        "the probe no longer feeds the planner, so the branch is being chosen by "
        "something other than the inputs a reader can check"
    )


def test_the_embedding_branch_is_planned_from_inputs_and_proved_on_the_host():
    """The choice between file, baseline and refusal must be a testable function.

    WHY a shape gate on a decision: the arm cannot run the engine on the host --
    `fs_open` and `fs_read_at` are syscalls, which is exactly why `weights.rs`
    says the host suite cannot drive `ModelFileDescriptor`. So the branch is what
    the host can prove, and it is provable only if picking it is a pure function
    of the inputs rather than a chain of conditions tangled into the reply.
    """
    lib = lib_source()
    planner = function_body(lib, "fn plan_embedding_run(")
    for input_name in ("weights_present", "token_id_count", "strict_neural_required"):
        assert input_name in planner, f"the planner no longer reads {input_name}"
    assert "probed_models" not in planner, "the planner reaches past its inputs at the probe"
    assert "fs_open" not in planner and "read_token_ids" not in planner, (
        "the planner opens files or parses payloads: the decision is no longer a "
        "function of the inputs and nothing can prove which branch answers"
    )
    for branch in ("RunInstalledFile", "LexicalBaseline", "Refuse"):
        assert branch in planner, f"the planner lost the {branch} branch"

    # And the host proves it, from the inputs, case by case.
    proofs = (
        "fn test_the_embedding_branch_is_chosen_from_the_inputs",
        "fn test_a_file_present_is_never_by_itself_a_neural_answer",
        "fn test_a_token_id_stream_is_read_or_refused_whole",
        "fn test_the_arm_answers_the_baseline_labelled_even_with_a_file_installed",
        "fn test_strict_neural_with_text_refuses_and_names_the_tokenizer_gap",
        "fn test_strict_neural_with_no_file_is_refused_as_unavailable",
        "fn test_a_token_id_stream_takes_the_file_backed_branch_not_the_baseline",
        "fn test_a_prompt_cut_in_transit_is_refused_with_both_byte_counts",
        "fn test_a_token_id_stream_cut_in_transit_is_refused_before_it_is_read",
        "fn test_the_cut_is_read_from_the_declaration_against_the_bytes",
        "fn test_a_loader_refusal_maps_to_the_status_that_names_it",
    )
    for proof in proofs:
        assert proof in lib, f"the branch-selection proof {proof!r} is gone"


def test_a_request_cut_in_transit_is_refused_before_its_bytes_are_mapped():
    """A message is cut at sixty-four bytes, and the half of a prompt that survives
    is not the prompt.

    WHY this needs a shape gate rather than only the host tests: the arm parses a
    twenty-four byte `AiInferenceRequest` out of the payload and only trusts it when
    the declared input length fits the bytes that arrived. A longer prompt therefore
    arrives as a whole header plus a fragment, the header is not trusted, and the
    fallback the arm has always had -- answer the whole payload as the query -- hands
    the tokenizer the caller's `model_id`, `target_dim`, `input_len`, `flags` and
    `input_offset` as if they were prose. On a device holding the model that is real
    arithmetic over the wrong bytes, reported as this request's embedding, and the
    host cannot see it happen because no host case can open the file. So the gate
    pins the position: the cut is named before a single byte of the payload can be
    read as ids or mapped as text.
    """
    lib = without_comments(lib_source())
    arm = embedding_arm(lib)

    guard = arm.index("declared_input_overrun(payload)")
    assert guard < arm.index("read_token_ids(query_text)"), (
        "the guard runs after the payload was already read as a token-id stream, so "
        "a prefix of a caller's ids still runs as if it were the whole stream"
    )
    assert guard < arm.index("encode_with_installed_file("), (
        "the guard runs after the mapping attempt, so the header's own bytes are "
        "segmented into ids -- and a cut prompt then earns the file-backed branch"
    )
    assert "EmbeddingGap::InputTruncated(declared, arrived)" in arm, (
        "the arm no longer refuses a request it could not have received whole"
    )

    refusal = function_body(lib, "fn embedding_gap_notice<'a>(")
    assert "InputTruncated" in refusal, "the new gap has no words of its own"
    cut = function_body(refusal, "EmbeddingGap::InputTruncated(declared, arrived) =>")
    assert "byte_count(declared as u64)" in cut and "byte_count(arrived as u64)" in cut, (
        "the refusal stopped naming both counts, which is the only thing the caller "
        "can act on -- it knows its prompt's length and cannot see the transport's"
    )
    assert "STATUS_UNSUPPORTED" in cut, "a refusal invented a status of its own"
    assert "STATUS_SUCCESS" not in cut, "a request that never arrived answered as one"

    # One predicate, two users: the guard may only fire on a payload the arm's own
    # parse would have trusted, or prose starts being refused for a header it never
    # sent. Sharing the clauses is what keeps the pair from drifting apart.
    shape = function_body(lib, "fn payload_is_header_shaped(")
    assert "req.model_id <= 16" in shape, "the shape test no longer reads the model id"
    parse = function_body(lib, "pub fn handle_ipc_inference_query(")
    assert "payload_is_header_shaped(&req)" in parse, (
        "the arm's parse grew its own copy of the clauses, so the guard and the "
        "parse can disagree about which bytes are a request"
    )


# The generative route's cut guard, and the empty strict-neural request beside it.
#
# WHY either needs a shape gate the way the embedding cut does: both guards are a
# position, and a position is the only thing they have. Their job is to run before
# a parse further down the same arm -- `extract_prompt_text` clamps a declared
# prompt to the prefix that arrived, and the embedding parse hands `query_text`
# the header's own twenty-four bytes when `input_len` is zero -- and a host test
# can say what a function returns, never where the arm calls it. Past the guard is
# where the device reads real weights, which is precisely the case no host can
# run, so the order is pinned here as a shape and not left to be inferred from
# green tests.
#
# WHY a detector that returns faults instead of a list of asserts: the self-test
# below runs the same reading against a source with a guard moved, deleted, or
# pointed at the other route's clauses. A second copy of these expressions there
# would be a second thing that can drift, and an assert stops at the first fault
# and hides the rest.


def tolerant_body(source: str, signature: str) -> str:
    """`function_body` for a source that may not hold the function at all.

    WHY the difference matters: a mutation that deletes a whole guard has to be
    reported as the fault it is, not raise before the detector says anything.
    """
    try:
        return function_body(source, signature)
    except (AssertionError, ValueError):
        return ""


def _generative_cut_faults(lib: str) -> list:
    """Every way the generative arm can go back to answering a prompt it never got whole."""
    faults = []
    arm = without_comments(generative_arm(lib))

    guard = "declared_prompt_overrun(payload)"
    if guard not in arm:
        return [f"the generative arm no longer asks whether the prompt arrived: {guard!r} is gone"]
    at = arm.index(guard)

    for site, why in (
        (
            "ACTIVE_GENERATION_TASK.store(sender_task_id",
            "a request that never fully arrived started a session the device can be "
            "asked to cancel",
        ),
        (
            "extract_prompt_text(payload)",
            "the parse clamps the declaration to the prefix, so every site below it "
            "already believes it holds a whole prompt",
        ),
        (
            "std::fs_open(path, std::FS_READ)",
            "the model file was opened and the prefix answered from before the cut "
            "was named",
        ),
    ):
        if site not in arm:
            faults.append(f"{site!r} is no longer in the arm, so the guard's position cannot be read")
        elif at > arm.index(site):
            faults.append(f"the cut guard runs after {site!r}: {why}")

    block = tolerant_body(arm, "if let Some((declared, arrived)) = " + guard + " {")
    if "generation_overrun_notice(declared, arrived" not in block:
        faults.append("the guard answers without the refusal that names both byte counts")
    if "reply_with_answer(sender_task_id, status, notice, tx_buf)" not in block:
        faults.append("the refusal is not handed to the route that streams a generative answer")
    if "STATUS_SUCCESS" in block:
        faults.append("a prompt that never arrived is answered as a success")

    notice = tolerant_body(lib, "fn generation_overrun_notice<'a>(")
    for counting in ("byte_count(declared as u64)", "byte_count(arrived as u64)"):
        if counting not in notice:
            faults.append(
                f"the refusal no longer names {counting!r} -- the caller knows how long "
                "its prompt was and cannot see what the transport did to it"
            )
    if "Nothing was answered from the piece that arrived" not in notice:
        faults.append("the refusal stopped saying the prefix went unanswered")
    if "STATUS_UNSUPPORTED" not in notice:
        faults.append("the refusal carries no status of its own")
    if "STATUS_SUCCESS" in notice:
        faults.append("a cut prompt answers with a success")

    # The sentence this route writes is wider than the mailbox keeps, so the route
    # that carries it is the one that streams.
    delivery = tolerant_body(lib, "fn reply_with_answer(")
    if "stream_answer(sender_task_id, status, MSG_TYPE_GENERATE_CHUNK, message)" not in delivery:
        faults.append(
            "a generative answer no longer travels the channel the reader drains, so a "
            "refusal arrives as the first forty bytes of itself"
        )

    # Guard and parse must share their clauses, and each route keeps its own.
    guard_shape = tolerant_body(lib, "fn declared_prompt_overrun(")
    if "declared_overrun(payload, generative_payload_is_header_shaped)" not in guard_shape:
        faults.append("the generative guard no longer measures with the clauses the generative parse trusts")
    parse = tolerant_body(lib, "fn extract_prompt_text(")
    if "generative_payload_is_header_shaped(&req)" not in parse:
        faults.append(
            "the parse grew clauses of its own, so guard and parse can disagree about "
            "which bytes are a request -- and prose starts being refused for a header "
            "nobody sent"
        )
    clauses = tolerant_body(lib, "fn generative_payload_is_header_shaped(")
    for clause in ("req.model_id <= 16", "req.flags <= 255"):
        if clause not in clauses:
            faults.append(f"the generative shape test no longer reads {clause!r}")
    if "target_dim" in clauses:
        faults.append(
            "a generation names no output dimension, so a guard that reads one refuses "
            "requests its own parse would answer"
        )
    embedding_guard = tolerant_body(lib, "fn declared_input_overrun(")
    if "declared_overrun(payload, payload_is_header_shaped)" not in embedding_guard:
        faults.append("the embedding guard stopped measuring with its own route's clauses")
    return faults


def _empty_strict_neural_faults(lib: str) -> list:
    """Every way a strict-neural request carrying nothing gets answered again."""
    faults = []
    arm = embedding_arm(without_comments(lib))

    guard = "strict_neural_request_is_empty(payload)"
    if guard not in arm:
        return [f"the embedding arm no longer asks what a strict request carries: {guard!r} is gone"]
    at = arm.index(guard)

    for site, why in (
        (
            "read_token_ids(query_text)",
            "the header's own bytes were already read as the caller's token ids",
        ),
        (
            "encode_with_installed_file(",
            "the mapping ran over the header, so a request that carried nothing earns "
            "the file-backed branch",
        ),
        (
            "plan_embedding_run(",
            "the branch was chosen from the probe, and an empty request gave the file "
            "nothing to run on either way",
        ),
    ):
        if site not in arm:
            faults.append(f"{site!r} is no longer in the arm, so the guard's position cannot be read")
        elif at > arm.index(site):
            faults.append(f"the empty-request guard runs after {site!r}: {why}")

    block = tolerant_body(arm, "if " + guard + " {")
    if "EmbeddingGap::EmptyRequest" not in block:
        faults.append("an empty strict-neural request is refused as some other gap")
    if "emit_embed_notice(sender_task_id, status, notice, tx_buf)" not in block:
        faults.append("the empty-request refusal is not delivered on the route that streams")

    notice = tolerant_body(lib, "fn embedding_gap_notice<'a>(")
    empty = tolerant_body(notice, "EmbeddingGap::EmptyRequest =>")
    if "neither text nor token ids" not in empty:
        faults.append("the refusal no longer says what the request did not carry")
    if "Dropping the flag is not a way past it" not in empty:
        faults.append(
            "the refusal promises the labelled baseline again, and a request with no "
            "query has no baseline answer either -- so the line would point the caller "
            "at a reply that does not exist"
        )
    if "STATUS_UNSUPPORTED" not in empty:
        faults.append("the empty-request refusal carries no status of its own")
    if "STATUS_SUCCESS" in empty:
        faults.append("a request that carried nothing answers with a success")

    predicate = tolerant_body(lib, "fn strict_neural_request_is_empty(")
    for clause in (
        "payload.len() != hdr",
        "FLAG_STRICT_NEURAL",
        "req.input_len == 0",
        "payload_is_header_shaped(&req)",
    ):
        if clause not in predicate:
            faults.append(f"the empty-request predicate no longer reads {clause!r}")
    return faults


def test_a_generative_prompt_cut_in_transit_is_refused_before_the_run_starts():
    """A prompt the transport cut must not become an answer to its own prefix.

    Sixty-four bytes is what an IPC message carries, and the kernel truncates
    rather than refusing. A generation whose prompt was longer than that arrives
    here as a whole header plus a fragment; `extract_prompt_text` clamps the
    declared length to the bytes it has and hands back the prefix, and every site
    below it -- the file open, the metadata read, the engine -- then answers the
    piece that survived as though the caller had asked that question. For an
    embedding the reader at least sees a vector of the width they asked for; a
    generation is fluent prose either way, so nothing downstream can spot the cut
    and this arm is the only party that still can.
    """
    faults = _generative_cut_faults(lib_source())
    assert not faults, faults


def test_an_empty_strict_neural_request_is_refused_as_empty():
    """A request carrying nothing is not a request carrying its own header.

    With `input_len` zero, the embedding parse falls back to the whole payload,
    and on a buffer that is only the header that leaves `model_id`, `target_dim`,
    `input_len`, `flags` and `input_offset` as the query. On a device holding the
    installed file those bytes segment into ids with no complaint -- they are
    bytes -- and the vector comes back filed under this caller as the model's
    reading of a request about itself. The caller sent nothing, so the only
    honest reply is that nothing reached the service.
    """
    faults = _empty_strict_neural_faults(lib_source())
    assert not faults, faults


def test_the_generative_and_empty_request_gates_can_fail():
    """A contract that cannot fail is a comment.

    Each block puts one fault back into a copy of the source and runs the *same*
    detector the gate above runs. `mutate_function` refuses an `old` it cannot
    find, so an arm whose target has moved fails here rather than going quiet.
    """
    lib = lib_source()
    dispatch = "pub fn handle_ipc_inference_query("

    clean = _generative_cut_faults(lib)
    assert not clean, f"the cut detector fires on the service as it stands: {clean}"
    clean_empty = _empty_strict_neural_faults(lib)
    assert not clean_empty, f"the empty-request detector fires as it stands: {clean_empty}"

    # The guard's own lines, read from the source rather than remembered here: an
    # arm that carried a hard copy of them would fail for a reformat, and a
    # failure that is not the fault it names is the one thing a mutation test is
    # not allowed to produce.
    def block_of(source, opener):
        start = source.index(opener)
        line_start = source.rfind("\n", 0, start) + 1
        indent = source[line_start:start]
        assert indent.strip() == "", f"{opener!r} is not the first thing on its line"
        end = source.index("\n" + indent + "}\n", start) + len("\n" + indent + "}\n")
        return source[line_start:end]

    cut_block = block_of(lib, "if let Some((declared, arrived)) = declared_prompt_overrun(payload) {")
    empty_block = block_of(lib, "if strict_neural_request_is_empty(payload) {")

    # 1. The guard moved past the registration, so a request that never fully
    #    arrived starts a cancellable session before it is refused.
    moved = mutate_function(lib, dispatch, cut_block, "")
    moved = mutate_function(
        moved, dispatch, "struct ActiveTaskGuard;", cut_block + "        struct ActiveTaskGuard;"
    )
    order = _generative_cut_faults(moved)
    assert any("ACTIVE_GENERATION_TASK" in fault for fault in order), (
        f"a guard behind the registration reads as a guard in front of it: {order}"
    )

    # 2. The guard kept in place but its refusal swallowed, so the cut is detected
    #    and answered from anyway.
    swallowed = mutate_function(
        lib, dispatch, "reply_with_answer(sender_task_id, status, notice, tx_buf);", "let _ = (status, notice);"
    )
    assert any("streams a generative answer" in fault for fault in _generative_cut_faults(swallowed))

    # 3. One of the two counts dropped from the sentence.
    one_count = mutate_function(
        lib, "fn generation_overrun_notice<'a>(", "notice.byte_count(arrived as u64);", ""
    )
    assert any("byte_count(arrived as u64)" in fault for fault in _generative_cut_faults(one_count)), (
        "a refusal naming only the declaration leaves the caller guessing what would have fit"
    )

    # 4. The route's clauses swapped: the generative guard reads the embedding
    #    arm's shape, which also demands a known output dimension.
    crossed = mutate_function(
        lib,
        "fn declared_prompt_overrun(",
        "declared_overrun(payload, generative_payload_is_header_shaped)",
        "declared_overrun(payload, payload_is_header_shaped)",
    )
    assert any("clauses the generative parse trusts" in fault for fault in _generative_cut_faults(crossed))

    # 5. A dimension demanded of a route that has no output width to name.
    widened = mutate_function(
        lib,
        "fn generative_payload_is_header_shaped(",
        "(req.model_id <= 16) && (req.flags <= 255)",
        "(req.model_id <= 16) && (req.flags <= 255) && (req.target_dim == 768)",
    )
    assert any("no output dimension" in fault for fault in _generative_cut_faults(widened))

    # 6. The answer put back on the mailbox alone, where it is cut at forty bytes.
    inline = mutate_function(
        lib,
        "fn reply_with_answer(",
        "stream_answer(sender_task_id, status, MSG_TYPE_GENERATE_CHUNK, message)",
        "false",
    )
    assert any("the channel the reader drains" in fault for fault in _generative_cut_faults(inline))

    # 7. The empty-request guard deleted, so a header-only request is answered as
    #    a prompt about its own header.
    gone = mutate_function(lib, dispatch, empty_block, "")
    gone_faults = _empty_strict_neural_faults(gone)
    assert any("no longer asks what a strict request carries" in fault for fault in gone_faults), (
        f"a deleted guard leaves the arm with nothing to say about an empty request: {gone_faults}"
    )

    # 8. The guard left in place but placed after the ids, the mapping and the
    #    plan -- which is where a host test still sees it refuse.
    late = mutate_function(lib, dispatch, empty_block, "")
    late = mutate_function(
        late,
        dispatch,
        "let plan = apply_file_vocabulary(plan, &text_ids);",
        empty_block + "    let plan = apply_file_vocabulary(plan, &text_ids);",
    )
    late_faults = _empty_strict_neural_faults(late)
    for site in ("read_token_ids(query_text)", "encode_with_installed_file(", "plan_embedding_run("):
        assert any(site in fault for fault in late_faults), (
            f"a guard behind {site!r} still reads as one in front of it: {late_faults}"
        )

    # 9. The empty request refused for the file it did not get rather than for
    #    what it did not send.
    mislabelled = mutate_function(
        lib, dispatch, "embedding_gap_notice(EmbeddingGap::EmptyRequest, &mut scratch)",
        "embedding_gap_notice(EmbeddingGap::NoWeights, &mut scratch)",
    )
    assert any("refused as some other gap" in fault for fault in _empty_strict_neural_faults(mislabelled))

    # 10. The neighbour's sentence recycled: "drop the flag" promises a baseline
    #     answer that an empty request has no query to reach.
    promised = mutate_function(
        lib, "fn embedding_gap_notice<'a>(", "Dropping the flag is not a way past it",
        "Drop the strict-neural flag to take the labelled lexical baseline",
    )
    assert any("no baseline answer" in fault for fault in _empty_strict_neural_faults(promised))

    # 11. The predicate widened to any payload at least a header long, so prose
    #     that happens to be twenty-four bytes starts being refused.
    broadened = mutate_function(
        lib, "fn strict_neural_request_is_empty(", "payload.len() != hdr", "payload.len() < hdr"
    )
    assert any("payload.len() != hdr" in fault for fault in _empty_strict_neural_faults(broadened))


def test_the_file_backed_branch_reaches_the_loader_it_was_written_for():
    """The engine hop the loader exists to close, taken from the dispatch arm.

    WHY this is a gate and not a comment: `weights.rs` was written, measured and
    left unreachable, and every one of its host numbers stayed true while the
    device answered from a hash. The claim "the service can run the installed
    file" is only worth anything if the arm that answers a query is the code that
    does it -- and only through the device's own positioned read, not a fixture.
    """
    lib = lib_source()
    run = function_body(lib, "fn run_installed_embedding(")
    for shipping in (
        "weights::ModelFileDescriptor",
        "weights::TensorTable::walk",
        "gguf::read_model_info_stream",
        "ModelConfig::from_gguf",
        "weights::WeightReader::new",
        "reader.load_final_norm",
        "Scratch::new(&config)",
        "KvCache::new(&config)",
        "weights::run_streamed_embedding(",
    ):
        assert shipping in run, f"the file-backed branch no longer uses {shipping!r}"
    assert "GgufStream::buffered_from_file(fd)" in run, (
        "the table is walked from something other than the ranged-read descriptor the guest opened"
    )
    assert "HostFileSource" not in run and "from_slice" not in run, (
        "the device path reads a host fixture; the loader's host measurement would "
        "no longer be a measurement of the code that ships"
    )
    assert "std::fs_close(fd)" in run, "the run leaks the descriptor it opened"
    # The refusal the loader returns is answered, not swallowed.
    assert "Err(error) =>" in embedding_arm(lib)
    assert "file_embed_notice(&error" in embedding_arm(lib)


def test_the_service_does_not_pass_its_own_id_guess_for_the_models_vocabulary():
    """The tokenizer gap is named, not filled with the built-in table.

    WHY this needs a gate more than anything else here: `tokenizer::encode` looks
    like the missing piece and is not one. It matches thirty hard-coded subwords
    and otherwise emits `4 + byte`; not one of those ids was ever checked against
    the `tokenizer.ggml.tokens` array the installed file states, which the reader
    deliberately does not walk. Wiring it into the file-backed branch would run
    real arithmetic over real weights for tokens that mean something else and
    report it as the model's embedding of the prompt -- a worse lie than the
    baseline it would have replaced, and one every other gate here would still
    pass. So the tempting wrong fix is the one this pins shut.
    """
    lib = without_comments(lib_source())
    run = function_body(lib, "fn run_installed_embedding(")
    assert "tokenizer::encode" not in run and "encode(" not in run, (
        "the built-in id table now feeds the file-backed run: the ids are not the "
        "file's vocabulary's, so the vector is not the prompt's embedding"
    )
    arm = embedding_arm(lib)
    assert "read_token_ids(query_text)" in arm, (
        "the ids no longer come from the caller, so something else is inventing them"
    )
    refusal = function_body(lib, "fn embedding_gap_notice<'a>(")
    assert "NoVocabTokenizer" in refusal
    assert "vocabulary" in refusal, "the refusal stopped naming the mapping it lacks"
    # The status the refusal earns is the unsupported one: a gap in the platform,
    # not a bad file and not a missing install.
    assert "STATUS_UNSUPPORTED" in refusal
    assert "STATUS_SUCCESS" not in refusal


def test_every_embedding_refusal_earns_its_own_code():
    """Three refusals with one code leave the reader guessing what would help.

    A dtype the engine has no layout for, a heap that refused the cache and a
    copy that stopped mid-tensor each have a different next move -- change the
    file, resize the run, re-install. The old arm flattened all of it into one
    number, so this checks the refusals are still distinguishable and that none
    of them was quietly promoted to a success.
    """
    notice = function_body(lib_source(), "fn file_embed_notice<'a>(")
    for code in ("STATUS_UNSUPPORTED", "STATUS_UNAVAILABLE", "STATUS_ERROR"):
        assert code in notice, f"the refusals no longer report {code}"
    assert "STATUS_SUCCESS" not in notice, "a refusal answered with a success"
    for named in (
        "UnsupportedDtype",
        "MixedBlockDtype",
        "TensorAbsent",
        "BadShape",
        "BadAlignment",
        "ShortRead",
        "ReadRefused",
        "OverDeliveredChunk",
        "ScratchTooLarge",
        "ScratchUnavailable",
        "ModelDeclaration",
    ):
        assert named in notice, f"the refusal for {named} no longer says which one it is"


def test_the_installed_file_is_considered_for_embeddings():
    """The install path writes one name, and the embedding probe must read it.

    The file the Agent Manager installs lands at `PATH_LOCAL_MODEL_DATA`. That
    path was already a generative candidate, so a person could install a model,
    watch the device accept it, and still be told -- by the embedding arm and by
    the search arm's `is_neural` -- that there are no weights, in the same minute
    the screen had offered to remove the file it was holding.
    """
    gguf = read("gguf.rs")
    probe = function_body(gguf, "pub fn probe_embedding_model(")
    assert "PATH_LOCAL_MODEL_DATA," in probe, (
        "the installed file is not an embedding candidate again"
    )
    assert probe.index("PATH_EMBEDDING_MODEL_DATA") < probe.index("PATH_LOCAL_MODEL_DATA"), (
        "the installed file now outranks the paths the embedding family owns, so a "
        "generative file renamed into that slot would be read as an embedding model"
    )


def test_the_baseline_status_is_its_own_number_and_not_a_synonym_for_success():
    """A new status only says something if a reader can branch on it.

    `0` meant success everywhere; if the baseline shared it the label would be
    decoration. This pins the code apart, and pins that nothing renumbered the
    five codes apps already read.
    """
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs").read_text(encoding="utf-8")
    codes = dict(
        (name, int(value))
        for name, value in re.findall(
            r"pub const (STATUS_[A-Z_]+): u32 = (\d+);", sdk
        )
    )
    assert codes["STATUS_SUCCESS"] == 0
    assert codes["STATUS_BASELINE_LEXICAL"] == 5, (
        f"the baseline shares a code with an existing status: {codes}"
    )
    assert len(set(codes.values())) == len(codes), f"two statuses share a number: {codes}"
    assert "FLAG_INPUT_TOKEN_IDS: u32 = 0x10" in sdk, (
        "the token-id flag moved, so a caller can no longer ask for the file-backed branch"
    )
    assert "FLAG_STRICT_NEURAL: u32 = 0x80" in sdk, (
        "the strict flag moved, so the demand-for-weights bit is now someone else's"
    )


def test_the_loader_no_longer_claims_the_service_never_calls_it():
    """`weights.rs` opens by saying what is not wired; that sentence is now false.

    WHY a gate on a doc comment: a module that says "no dispatch arm calls into
    this module yet" while an arm calls it on every token-id request is the exact
    shape of the defect this repository exists to catch, and a reader who trusts
    the header will trust the wrong thing about the device. The replacement
    sentence has to keep the half that is still a limitation.
    """
    doc = " ".join(
        " ".join(
            line[3:].strip()
            for line in read("weights.rs").splitlines()
            if line.startswith("//!")
        ).split()
    )
    assert "no dispatch arm" not in doc, "the header still denies the wiring that happened"
    for kept in (
        "never been executed by the guest",
        "STATUS_BASELINE_LEXICAL",
        "tokenizer::encode",
    ):
        assert kept in doc, f"the header stopped saying {kept!r}"
    header = chr(10).join(
        line
        for line in read("weights.rs").splitlines()
        if not line.startswith("// SPDX-")
    ).lstrip()
    assert header.startswith("//! "), (
        "the module's own words moved out of the header"
    )


# ---------------------------------------------------------------------------
# The tensor reader's own count of the quantisations it can lay a row out for.
# ---------------------------------------------------------------------------

# The sentence `weights.rs` puts above its loader error type, naming how many
# quantisations the arithmetic knows. It is required to exist: a loader whose
# capability is unnumbered is a loader no reader can check.
QUANTISATION_CLAIM = re.compile(
    r"(\w+)\s+quantisations\s+`gemv` and\s+`embedding_row_into` know"
)

WORD_NUMBERS = {
    "one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7,
    "eight": 8, "nine": 9, "ten": 10, "eleven": 11, "twelve": 12,
}


def mapped_dtype_ids(inference: str) -> list:
    """The GGML dtype ids `quantised_row_bytes` gives a row layout to.

    WHY the arms are read out of the `match` and not counted from a comment: that
    match is the function the loader calls to size every tensor row, so its arm
    list is the number a refusal is actually made of.
    """
    body = function_body(inference, "pub fn quantised_row_bytes(")
    arms = body[body.index("match dtype {"):]
    return [int(n) for n in re.findall(r"^[ \t]+(\d+) =>", arms, re.M)]


def quantisation_claim(weights: str) -> tuple:
    """`(count, paragraph)` for the sentence naming the reader's quantisations.

    The paragraph is the whole `///` block the sentence opens, because the ids it
    lists and the refusal it promises live in the lines under it.
    """
    lines = weights.splitlines()
    for index, line in enumerate(lines):
        match = QUANTISATION_CLAIM.search(line)
        if not match:
            continue
        paragraph = []
        for follow in lines[index:]:
            if not follow.lstrip().startswith("///"):
                break
            paragraph.append(follow.lstrip()[3:].strip())
        word = match.group(1).lower()
        count = int(word) if word.isdigit() else WORD_NUMBERS.get(word)
        return count, " ".join(part for part in paragraph if part)
    raise AssertionError(
        "the tensor reader no longer states how many quantisations it knows the "
        "byte layout of, so nothing can check the claim against the engine"
    )


def ids_named(paragraph: str, ids: list) -> list:
    """Which of `ids` the paragraph names, by number rather than by adjective."""
    return [dtype for dtype in ids if re.search(rf"\b{dtype}\b", paragraph)]


def test_the_reader_names_the_quantisations_the_engine_maps():
    """`weights.rs` claims eight quantisations; the engine sizes a row for five.

    The ids `quantised_row_bytes` maps are 0 F32, 1 F16, 2 Q4_0, 8 Q8_0 and 12
    Q4_K, and every other dtype is refused by its number --
    `InferenceError::UnsupportedQuantization` and `LoadError::UnsupportedDtype`.
    Q4_1, Q5_K and Q6_K are not implemented anywhere in this service. A count of
    eight is therefore a capability the build does not have, printed in the one
    file a reader opens to learn what the loader can read: the defect the
    Platform Truth Contract exists to stop, and a doc-only fix.
    """
    inference = read("inference.rs")
    ids = mapped_dtype_ids(inference)
    claimed, paragraph = quantisation_claim(read("weights.rs"))

    assert claimed is not None, "the reader's quantisation count is not a number"
    assert claimed == len(ids), (
        f"the reader claims {claimed} quantisations; `quantised_row_bytes` maps "
        f"{len(ids)}: {ids}"
    )
    # A count alone is still a number no reader can verify, so the same block has
    # to name the ids it counted.
    assert ids_named(paragraph, ids) == ids, (
        f"the claim counts {len(ids)} quantisations without naming all of {ids}"
    )
    # And it has to say what happens to a dtype it has not mapped: refused by its
    # number, never read as a neighbour's layout.
    assert "number" in paragraph, (
        "the claim no longer says an unmapped dtype is refused by its number"
    )


def test_the_quantisation_claim_gate_can_fail():
    """A gate nobody has seen fail is a comment.

    Each block puts one fault back into a copy of the two files and runs the
    *same* detector the gate above runs, so the count, the enumeration and the
    refusal are each proved still to be observable.
    """
    inference = read("inference.rs")
    weights = read("weights.rs")
    ids = mapped_dtype_ids(inference)

    # 1. The claim as it shipped: eight named over a five-id engine.
    inflated = weights.replace("The five quantisations", "The eight quantisations", 1)
    assert inflated != weights, "the reader's count sentence moved; this fixture is stale"
    claimed, _ = quantisation_claim(inflated)
    assert claimed != len(ids), (
        "claiming eight over the ids the engine maps no longer parts the two numbers"
    )

    # 2. The engine gains a row layout and the comment is not told. The gate has
    # to see the two numbers part company from this side too, or it only ever
    # checks that nobody re-typed a word.
    widened = inference.replace(
        "        8 => blocks(32, 34),",
        "        8 => blocks(32, 34),\n        3 => blocks(32, 144),",
        1,
    )
    assert widened != inference, "the dtype match moved; this fixture no longer matches it"
    grown = mapped_dtype_ids(widened)
    assert len(grown) == len(ids) + 1, "the detector stopped counting the match's arms"
    claimed, _ = quantisation_claim(weights)
    assert claimed != len(grown), (
        "a sixth arm would keep a five-id claim passing"
    )

    # 3. The count stays right while the enumeration goes: the reader would be
    # back to asserting a number a reader cannot check.
    unlisted = weights.replace(
        "dtype ids 0 (F32), 1 (F16), 2 (Q4_0), 8 (Q8_0) and 12 (Q4_K)",
        "the dtype ids the engine names",
        1,
    )
    assert unlisted != weights, "the reader's id list moved; this fixture is stale"
    _, paragraph = quantisation_claim(unlisted)
    assert ids_named(paragraph, ids) != ids, (
        "the gate would pass a claim that names none of the ids it counts"
    )

    # 4. The refusal sentence dropped, so the paragraph implies a guess instead
    # of a refusal.
    unrefused = weights.replace(
        "naming the tensor and the number.", "naming the tensor.", 1
    )
    assert unrefused != weights, "the refusal sentence moved; this fixture is stale"
    _, paragraph = quantisation_claim(unrefused)
    assert "number" not in paragraph, (
        "the gate would pass a claim that no longer says an unmapped dtype is refused"
    )

    # None of those faults are in the service as it stands.
    claimed, paragraph = quantisation_claim(weights)
    assert claimed == len(ids) == 5 and ids == [0, 1, 2, 8, 12]
    assert ids_named(paragraph, ids) == ids
    assert "number" in paragraph


def mutate_function(source: str, signature: str, old: str, new: str) -> str:
    """`source` with one occurrence of `old` inside the named function replaced.

    WHY mutations are scoped to a function: the gates below read a single body, and
    a whole-file `str.replace` can put the fault somewhere the gate never looks --
    where the meta-test then passes on a mutation that proves nothing. The assert is
    the point: an unfound `old` means the gate's target moved, not that the fault is
    unreachable.
    """
    body = function_body(source, signature)
    assert old in body, f"{old!r} is not inside {signature!r}: the fault has nowhere to go"
    return source.replace(body, body.replace(old, new, 1), 1)


def status_codes(sdk: str) -> list:
    return [int(value) for _, value in re.findall(
        r"pub const (STATUS_[A-Z_]+): u32 = (\d+);", sdk
    )]


def test_the_embedding_contract_can_fail():
    """A contract that cannot fail is a comment.

    Each block below puts one fault back into a copy of the source and runs the
    *same* detector expression the gate that would have caught it runs. A mutation
    that no assertion fires on is a gate that has nothing left to see, which is the
    failure this meta-test exists to find.
    """
    lib = lib_source()
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "ai.rs").read_text(encoding="utf-8")
    gguf = read("gguf.rs")

    def label_of(source):
        """The gate's own split: what status the arm may name before the file ran."""
        arm = without_comments(embedding_arm(source))
        return arm.split("EmbeddingPlan::RunInstalledFile =>", 1)

    # 1. The baseline answers with the weights status -- the defect as it shipped.
    relabelled = mutate_function(
        lib,
        "pub fn handle_ipc_inference_query(",
        "reply_lexical_baseline(sender_task_id, query_text, target_dim, latency_micros, tx_buf);",
        "let _v = embedding::compute_embedding_768(query_text, target_dim);\n            "
        "emit_response_as(sender_task_id, STATUS_SUCCESS, MSG_TYPE_EMBED, query_text, tx_buf);",
    )
    before, after = label_of(relabelled)
    assert "STATUS_SUCCESS" in before, (
        "the arm's status split no longer sees a success sent before the file ran"
    )
    assert "STATUS_SUCCESS" in after

    # 2. A second, unlabelled projection beside the labelled one.
    assert "compute_embedding_768" not in without_comments(embedding_arm(lib)), (
        "the live arm already projects inside itself, so the gate's shape rule is idle"
    )
    duplicated = mutate_function(
        lib,
        "pub fn handle_ipc_inference_query(",
        "let plan = plan_embedding_run(",
        "let _draft = embedding::compute_embedding_768(query_text, target_dim, None, None);\n    let plan = plan_embedding_run(",
    )
    assert "compute_embedding_768" in embedding_arm(without_comments(duplicated)), (
        "a projection written inside the arm goes unnoticed by the one-place rule"
    )

    # 3. The retrieval flag borrowed back off the probe.
    borrowed = mutate_function(
        lib,
        "pub fn handle_ipc_inference_query(",
        "rag::RagIndex::new(false)",
        "let is_neural = probed_models.embedding.is_some();\n            rag::RagIndex::new(is_neural)",
    )
    retrieval = search_arm(without_comments(borrowed))
    assert "rag::RagIndex::new(false)" not in retrieval, (
        "the detector for a probe-borrowed neural flag has nothing left to trip on"
    )
    assert "is_neural = probed_models.embedding.is_some()" in retrieval

    # 4. The wire field computed from the install instead of the index.
    relayed = mutate_function(
        lib,
        "pub fn handle_ipc_inference_query(",
        "is_neural: m.is_neural as u8",
        "is_neural: if probed_models.embedding.is_some() { 1 } else { 0 }",
    )
    assert "is_neural: m.is_neural as u8" not in search_arm(without_comments(relayed)), (
        "the relay the gate pins is no longer the thing it reads"
    )
    # The gate's site loop reads the dispatch, so a live-file check closes it.
    assert "if probed_models.embedding.is_some() { 1 } else { 0 }" not in lib

    # 5. The probe replaced by a constant, so the branch stops being a decision.
    decoupled = mutate_function(
        lib,
        "pub fn handle_ipc_inference_query(",
        "        probed_models.embedding.is_some(),\n        token_ids.map(|ids| ids.len),",
        "        true,\n        token_ids.map(|ids| ids.len),",
    )
    assert (
        "plan_embedding_run( probed_models.embedding.is_some(), token_ids.map(|ids| ids.len), strict_neural_required, )"
        not in flat(embedding_arm(without_comments(decoupled)))
    ), "the planner-input pin is satisfied by something other than the probe"

    # 6. The planner opens files, so no host test can drive it from inputs.
    reaching = mutate_function(
        lib,
        "fn plan_embedding_run(",
        "match (weights_present, token_id_count, strict_neural_required) {",
        "let weights_present = std::fs_open(path, std::FS_READ).is_ok();\n        "
        "match (weights_present, token_id_count, strict_neural_required) {",
    )
    assert "fs_open" in function_body(reaching, "fn plan_embedding_run("), (
        "a planner that reaches for the filesystem no longer reads as one"
    )

    # 7. A gap swallowed into a baseline answer, so nothing refuses.
    swallowed = lib
    for _ in range(3):
        swallowed = mutate_function(
            swallowed, "fn plan_embedding_run(",
            "=> EmbeddingPlan::Refuse(", "=> EmbeddingPlan::Renamed(",
        )
    assert "Refuse" not in function_body(swallowed, "fn plan_embedding_run("), (
        "the refusal branch the gate names is not the one the planner takes"
    )

    # 8. The wired loader swapped for a host fixture.
    mocked = mutate_function(
        lib, "fn run_installed_embedding(",
        "weights::ModelFileDescriptor { fd }", "HostFileSource { file: std::fs::File::open(path)? }",
    )
    run = function_body(mocked, "fn run_installed_embedding(")
    assert "weights::ModelFileDescriptor" not in run and "HostFileSource" in run, (
        "the file-backed branch would pass while reading a fixture"
    )

    # 9. The engine hop undone: the table walked, but no graph stepped.
    unwired = mutate_function(
        lib, "fn run_installed_embedding(",
        "weights::run_streamed_embedding(", "weights::run_streamed_prefix(",
    )
    assert "weights::run_streamed_embedding(" not in function_body(
        unwired, "fn run_installed_embedding("
    ), "the loader call the gate requires is not the call the branch makes"

    # 10. The tempting wrong fix: the built-in id table feeding the run.
    guessed = mutate_function(
        lib, "fn run_installed_embedding(",
        "    let fd = std::fs_open(path, std::FS_READ)",
        "    let mut invented = [0u32; 8];\n    tokenizer::encode(path, &mut invented, true);\n"
        "    let fd = std::fs_open(path, std::FS_READ)",
    )
    assert "tokenizer::encode" in function_body(guessed, "fn run_installed_embedding("), (
        "the vocabulary rule cannot see ids invented inside the run"
    )

    # 11. A refusal promoted to a success.
    promoted = mutate_function(
        lib, "fn file_embed_notice<'a>(", "STATUS_UNAVAILABLE", "STATUS_SUCCESS"
    )
    assert "STATUS_SUCCESS" in function_body(promoted, "fn file_embed_notice<'a>("), (
        "the no-success rule has nothing to read in the refusals"
    )

    # 12. The installed file dropped from, or moved to the front of, the probe.
    blind = mutate_function(
        gguf, "pub fn probe_embedding_model(", "        PATH_LOCAL_MODEL_DATA,\n", ""
    )
    assert "PATH_LOCAL_MODEL_DATA," not in function_body(blind, "pub fn probe_embedding_model("), (
        "the candidate rule cannot see the installed file being removed"
    )
    reordered = mutate_function(
        gguf, "pub fn probe_embedding_model(",
        "        PATH_EMBEDDING_MODEL_DATA,",
        "        PATH_LOCAL_MODEL_DATA,\n        PATH_EMBEDDING_MODEL_DATA,",
    )
    probe = function_body(reordered, "pub fn probe_embedding_model(")
    assert probe.index("PATH_EMBEDDING_MODEL_DATA") > probe.index("PATH_LOCAL_MODEL_DATA"), (
        "the order the gate pins is not the order it reads"
    )

    # 13. The baseline status renumbered onto an existing one.
    merged = sdk.replace(
        "pub const STATUS_BASELINE_LEXICAL: u32 = 5;",
        "pub const STATUS_BASELINE_LEXICAL: u32 = 0;", 1
    )
    assert status_codes(merged) != status_codes(sdk) and len(set(status_codes(merged))) < len(
        status_codes(merged)
    ), "two statuses sharing a number would still leave the code list unique"

    # 14. The header denying, again, the wiring that happened.
    denied = "//! no dispatch arm in `lib.rs` calls into this module yet.\n" + read("weights.rs")
    header = " ".join(
        " ".join(
            line[3:].strip()
            for line in denied.splitlines()
            if line.startswith("//!")
        ).split()
    )
    assert "no dispatch arm" in header, "the header rule no longer reads the lines it names"

    # None of the faults above are in the service as it stands.
    live_run = function_body(lib, "fn run_installed_embedding(")
    assert "HostFileSource" not in live_run and "tokenizer::encode" not in live_run
    assert "weights::run_streamed_embedding(" in live_run
    assert "PATH_LOCAL_MODEL_DATA," in function_body(gguf, "pub fn probe_embedding_model(")
    assert function_body(gguf, "pub fn probe_embedding_model(").index(
        "PATH_EMBEDDING_MODEL_DATA"
    ) < function_body(gguf, "pub fn probe_embedding_model(").index("PATH_LOCAL_MODEL_DATA")
    assert "STATUS_BASELINE_LEXICAL" in function_body(lib, "fn reply_lexical_baseline(")
    assert "STATUS_SUCCESS" not in function_body(lib, "fn file_embed_notice<'a>(")
    assert "Refuse" in function_body(lib, "fn plan_embedding_run(")
    assert "fs_open" not in function_body(lib, "fn plan_embedding_run(")
    assert "rag::RagIndex::new(false)" in search_arm(without_comments(lib))
    assert "is_neural: m.is_neural as u8" in search_arm(without_comments(lib))
    assert len(set(status_codes(sdk))) == len(status_codes(sdk))


# ---------------------------------------------------------------------------
# The embedding id stream: one bound, printed as the number the code enforces.
# ---------------------------------------------------------------------------

# The single place the run's id-stream ceiling is written down. It has to be a
# named constant on one line, because every other site that needs the bound is
# checked below for naming *it* rather than repeating its value.
ID_STREAM_BOUND = re.compile(r"const EMBED_TOKEN_IDS_MAX: usize = (\d+);")

# A `b"..."` literal is a sentence a notice prints. A digit inside one is a bound
# no constant owns, which is exactly how a raised ceiling keeps telling callers
# about the old one.
BYTE_LITERAL = re.compile(r'b"(?:[^"\\]|\\.)*"')

# Prose in the service's own headers that restates the bound, so a reader who
# never opens `lib.rs` cannot be handed a number the code stopped enforcing.
PROSE_BOUND = re.compile(r"bounded at (\d+) positions")


def id_stream_bound(lib: str) -> int:
    """The bound `EMBED_TOKEN_IDS_MAX` states, or `AssertionError` if it is gone."""
    match = ID_STREAM_BOUND.search(lib)
    assert match is not None, (
        "EMBED_TOKEN_IDS_MAX is no longer `const ... : usize = <number>;`, so the "
        "number a refusal prints has nothing to be compared against"
    )
    return int(match.group(1))


def id_stream_prose_sources() -> dict:
    """Every inference-service source, by file name, for the prose sweep below."""
    return {
        path.name: path.read_text(encoding="utf-8")
        for path in sorted(SERVICE.glob("*.rs"))
    }


def literals_with_digits(fragment: str) -> list:
    """The byte literals in `fragment` that contain a digit."""
    return [
        literal
        for literal in BYTE_LITERAL.findall(fragment)
        if any(character.isdigit() for character in literal)
    ]


def _id_stream_bound_faults(lib: str, tokenizer: str, prose: dict) -> list:
    """Every way the id-stream refusal can drift from the bound the array enforces."""
    faults = []
    match = ID_STREAM_BOUND.search(lib)
    if match is None:
        return [
            "EMBED_TOKEN_IDS_MAX is no longer a named constant on one line, so the "
            "number the refusal prints has nothing to be compared against"
        ]
    bound = int(match.group(1))

    # The buffer and the guard both have to name the constant: an array sized by a
    # literal would enforce one bound while the notice printed another, and the
    # refusal's whole claim is that the two are the same number.
    struct_body = tolerant_body(lib, "struct TokenIds {")
    if "ids: [u32; EMBED_TOKEN_IDS_MAX]," not in struct_body:
        faults.append("TokenIds no longer holds EMBED_TOKEN_IDS_MAX positions")

    reader = tolerant_body(lib, "fn read_token_ids(")
    if "if count > EMBED_TOKEN_IDS_MAX {" not in reader:
        faults.append("read_token_ids no longer refuses a stream at EMBED_TOKEN_IDS_MAX")
    if "let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];" not in reader:
        faults.append("read_token_ids sizes its own buffer with something other than EMBED_TOKEN_IDS_MAX")

    mapper = tolerant_body(lib, "fn encode_with_installed_file(")
    if "let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];" not in mapper:
        faults.append("the vocabulary mapping buffers something other than EMBED_TOKEN_IDS_MAX ids")

    # The two numbers the caller is shown have to be the segmentation's own, not
    # recomputed here: a refusal that guesses how many ids were needed is a refusal
    # that cannot be checked against the file.
    planner = tolerant_body(lib, "fn apply_file_vocabulary(")
    if "EmbeddingGap::IdStreamFull(*needed, *bound)" not in planner:
        faults.append("the refusal no longer carries the segmentation's own two numbers")
    if "IdStreamFull(usize, usize)," not in lib:
        faults.append("IdStreamFull no longer carries a pair of numbers")

    # The sentence the caller reads: both figures printed from the values handed to
    # the arm, the bound named in words, and nothing cut silently.
    notice = tolerant_body(lib, "fn embedding_gap_notice<'a>(")
    arm = tolerant_body(notice, "EmbeddingGap::IdStreamFull(needed, bound) =>")
    if not arm:
        faults.append("the id-stream refusal no longer has an arm of its own")
    else:
        for printed in ("notice.byte_count(needed as u64);", "notice.byte_count(bound as u64);"):
            if printed not in arm:
                faults.append(f"the refusal no longer prints the number from {printed!r}")
        if "one embedding run answers at most " not in arm:
            faults.append("the refusal no longer says what the bound is a bound on")
        if "Nothing was truncated" not in arm:
            faults.append("the refusal no longer says it truncated nothing")
        if "STATUS_UNSUPPORTED" not in arm:
            faults.append("the id-stream refusal answers with no status of its own")
        if "STATUS_SUCCESS" in arm:
            faults.append("a run that answered no vector reports success")
        numbered = literals_with_digits(arm)
        if numbered:
            faults.append(
                "the refusal hard-codes a count inside its own sentence, so the number "
                f"a caller reads is not the number the array enforces: {numbered}"
            )

    # `encode` reports the bound as the buffer's length, which is the constant: if
    # it ever reported a remembered number the pins above would agree with a lie.
    encoder = tolerant_body(tokenizer, "pub fn encode(&self, text: &[u8], out: &mut [u32])")
    if "if needed > out.len() {" not in encoder:
        faults.append("encode no longer measures a segmentation against the buffer it was handed")
    if "bound: out.len()," not in encoder:
        faults.append("encode reports a bound that is not the length of the buffer")

    # Prose restating the ceiling, in any header a reader opens instead of lib.rs.
    for name, text in sorted(prose.items()):
        for stated in PROSE_BOUND.findall(text):
            if int(stated) != bound:
                faults.append(
                    f"{name} tells its reader the run is bounded at {stated} positions "
                    f"while EMBED_TOKEN_IDS_MAX is {bound}"
                )

    # The gate's own proof: a host test that formats the notice and reads the figure
    # back, so the agreement is executed rather than only asserted about source shape.
    host = tolerant_body(
        lib, "fn test_the_id_stream_refusal_prints_the_bound_the_service_enforces("
    )
    if "b\"answers at most \"" not in host or "EMBED_TOKEN_IDS_MAX" not in host:
        faults.append(
            "no host test formats the id-stream refusal and compares the number it "
            "prints to EMBED_TOKEN_IDS_MAX"
        )
    return faults


def test_the_id_stream_refusal_prints_the_bound_the_run_enforces():
    """The 512-position bound is one number, and the caller is told exactly that one.

    The refusal at `EmbeddingGap::IdStreamFull` is the only place a person learns
    how long a stream the embedding run answers, and the array in `TokenIds` is the
    only thing that decides whether a stream fits. If those two can be made to say
    different numbers -- by typing a literal into the sentence, by resizing the
    array alone, or by re-typing the ceiling into a module header -- the service
    would refuse requests it could have answered, or promise room it does not have.
    Either is a false capability claim, so the constant, the buffer, the guard, the
    printed pair and the prose are pinned to each other here, and executed against
    each other by the host test this gate requires.

    WHY this does not also pin the value to the model's declared context: 32 ids is
    already 128 bytes and an IPC message carries 64 (`kernel/src/ipc/mod.rs:34`),
    24 of which the request header takes, so at most 10 ids reach this service whole
    today. Raising `EMBED_TOKEN_IDS_MAX` moves the wall from the run onto the
    transport without moving the caller; the bound stays where the transport puts it
    until the ids have a pipe to travel in on.
    """
    lib = lib_source()
    faults = _id_stream_bound_faults(lib, read("tokenizer.rs"), id_stream_prose_sources())
    assert not faults, faults


def test_the_id_stream_bound_gate_can_fail():
    """A bound nobody watched drift is a bound nobody moved on purpose.

    Each block puts one fault back into a copy and runs the *same* detector the gate
    above runs. The last two blocks are the ones that matter for a raised ceiling:
    the detector must fire on a hard-coded number, and must stay silent when a
    different bound is stated consistently everywhere.
    """
    lib = lib_source()
    tokenizer = read("tokenizer.rs")
    prose = id_stream_prose_sources()

    clean = _id_stream_bound_faults(lib, tokenizer, prose)
    assert not clean, f"the bound detector fires on the service as it stands: {clean}"
    assert id_stream_bound(lib) == 512, "the shipped bound moved without this fixture being re-read"

    # 1. The printed bound frozen to today's number while the constant moves. This
    # is the exact drift this gate exists to stop: the notice keeps saying 32, the
    # array keeps getting bigger, and the caller is told to shorten a text that
    # already fit.
    frozen = mutate_function(
        lib,
        "fn embedding_gap_notice<'a>(",
        'notice.word(b" token ids and one embedding run answers at most ");\n'
        "            notice.byte_count(bound as u64);",
        'notice.word(b" token ids and one embedding run answers at most ");\n'
        "            notice.byte_count(32);",
    )
    faults = _id_stream_bound_faults(frozen, tokenizer, prose)
    assert any(
        "notice.byte_count(bound as u64);" in f for f in faults
    ), f"a frozen bound in the notice passes the gate: {faults}"

    # 2. The count typed into the sentence itself, leaving the dynamic print in
    # place -- a half-fix that puts an unverifiable number in front of the caller.
    typed = mutate_function(
        lib,
        "fn embedding_gap_notice<'a>(",
        'notice.word(b" token ids and one embedding run answers at most ");',
        'notice.word(b" token ids and one embedding run answers at most 64 ids ");',
    )
    faults = _id_stream_bound_faults(typed, tokenizer, prose)
    assert any("hard-codes a count" in f for f in faults), (
        f"a digit inside the refusal's own sentence passes the gate: {faults}"
    )

    # 3. The guard moved to a literal, so the array and the refusal part company
    # from the enforcement side.
    ungated = mutate_function(
        lib, "fn read_token_ids(", "if count > EMBED_TOKEN_IDS_MAX {", "if count > 10 {"
    )
    faults = _id_stream_bound_faults(ungated, tokenizer, prose)
    assert any(
        "read_token_ids no longer refuses a stream at EMBED_TOKEN_IDS_MAX" in f for f in faults
    ), f"a literal read guard passes the gate: {faults}"

    # 4. The vocabulary buffer narrowed to the transport's reach while the printed
    # bound stayed at the run's: a caller told 32 would be refused at 10 by a
    # segmentation number that never reached the notice.
    narrowed = mutate_function(
        lib,
        "fn encode_with_installed_file(",
        "let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];",
        "let mut ids = [0u32; 10];",
    )
    faults = _id_stream_bound_faults(narrowed, tokenizer, prose)
    assert any(
        "buffers something other than EMBED_TOKEN_IDS_MAX" in f for f in faults
    ), f"a mapping buffer sized by a literal passes the gate: {faults}"

    # 5. Prose in the tokenizer's own header left behind by a raised ceiling.
    moved = {
        name: (
            text.replace("bounded at 512 positions", "bounded at 1024 positions", 1)
            if name == "tokenizer.rs"
            else text
        )
        for name, text in prose.items()
    }
    assert moved["tokenizer.rs"] != prose["tokenizer.rs"], (
        "the tokenizer header no longer restates the bound; this fixture is stale"
    )
    faults = _id_stream_bound_faults(lib, tokenizer, moved)
    assert any(
        "while EMBED_TOKEN_IDS_MAX is 512" in f for f in faults
    ), f"prose claiming 1024 positions passes the gate: {faults}"

    # 6. The host proof of the agreement deleted: the source shapes still line up,
    # but nothing any more runs the notice and reads its digits back.
    unproved = lib.replace(
        "fn test_the_id_stream_refusal_prints_the_bound_the_service_enforces(",
        "fn retired_the_id_stream_bound_proof(",
        1,
    )
    faults = _id_stream_bound_faults(unproved, tokenizer, prose)
    assert any("no host test formats" in f for f in faults), (
        f"the gate passes with the notice never formatted: {faults}"
    )

    # 7. A raised bound, stated consistently everywhere, must PASS. Without this
    # the detector would be pinning the number 512 rather than the agreement between
    # the number and the bound, and the day a pipe carries the ids it would be the
    # gate itself that refuses the raise.
    raised_lib = lib.replace(
        "const EMBED_TOKEN_IDS_MAX: usize = 512;",
        "const EMBED_TOKEN_IDS_MAX: usize = 1024;",
        1,
    )
    raised_prose = {
        name: (
            text.replace("bounded at 512 positions", "bounded at 1024 positions", 1)
            if name == "tokenizer.rs"
            else text
        )
        for name, text in prose.items()
    }
    raised = _id_stream_bound_faults(raised_lib, tokenizer, raised_prose)
    assert not raised, f"the gate freezes the value at 512 instead of the agreement: {raised}"
    assert id_stream_bound(raised_lib) == 1024


def main() -> int:
    tests = [name for name in globals() if name.startswith("test_")]
    failed = 0
    for name in sorted(tests):
        try:
            globals()[name]()
            print(f"PASS: {name}")
        except Exception as exc:  # noqa: BLE001 - a failing case is the report
            failed += 1
            print(f"FAIL: {name}: {exc}")
    print(f"\n{len(tests) - failed} of {len(tests)} cases pass.")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
