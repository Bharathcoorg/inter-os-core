"""Truth contract for the on-device inference service's generative path.

The service answered a chat prompt with words from weights it never loaded: a
forward pass over zero-filled tensors at the dimensions a unit test had picked,
reported with STATUS_SUCCESS. Every assertion below pins one half of the rule
that replaces it -- the model file on disk is the only thing allowed to size the
engine, and when the engine cannot be built from that file the reply says so.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SERVICE = ROOT / "userspace" / "system-service" / "src"

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
    helper = function_body(lib_source(), "fn emit_inference_response")
    assert "output_dim: message.len() as u32" in helper
    assert "WHY the header counts bytes and not tokens" in doc_comment(
        lib_source(), "fn emit_inference_response"
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
