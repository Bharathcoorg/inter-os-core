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
    # The refusals the walk cannot speak about keep their old 240-byte window.
    assert arm.count("let mut scratch = [0u8; 320];") == 2
    assert arm.count("let mut scratch = [0u8; 240];") == 2


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
    body = function_body(inference, "pub fn from_gguf")
    for clamp in (".min(", ".max(", "unwrap_or(MAX_"):
        assert clamp not in body, f"from_gguf clamps a dimension with {clamp!r}"
    assert "MAX_HIDDEN_DIM" in body


def test_the_head_geometry_is_checked_against_the_row_it_is_cached_in():
    """Each head is cached in a row 32 floats wide, and the heads are laid end
    to end across the hidden dimension."""
    inference = read("inference.rs")
    assert "pub const MAX_HEAD_DIM: usize = 32;" in inference
    assert "heads_span" in inference
    assert "attention.head_count x head_length" in inference


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
    assert "let mut scratch = [0u8; 240];" in generative_arm(lib)
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
    if "let mut scratch = [0u8; 320];" not in generative_arm(lib):
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
