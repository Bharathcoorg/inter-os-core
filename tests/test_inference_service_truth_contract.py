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
    assert arm.count("let mut scratch = [0u8; NOTICE_WINDOW_BYTES];") == 4
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
