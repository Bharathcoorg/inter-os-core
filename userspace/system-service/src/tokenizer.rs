// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The vocabulary the installed file declares, read out of the file and used.
//!
//! # WHAT THIS REPLACED, AND WHY THE OLD THING COULD NOT BE WIRED UP
//!
//! This module used to hold thirty `COMMON_SUBWORDS` at invented ids 260..=289
//! and a byte fallback at a hard-coded offset of 4. Nothing about that table came
//! from a file: the ids were typed in, and the model the device installs declares
//! a 262,144-entry vocabulary in which id 260 is `<unused4294>` and id 261 is
//! `<unused4295>`. Running the embedding graph on those numbers would have read
//! real tensors for tokens the model does not have and reported the result as the
//! prompt's embedding -- so the service refused to do it, and every text-only
//! request was answered from the lexical baseline. The refusal was correct. The
//! blocker was that the mapping did not exist.
//!
//! It exists now, and it is read from the file. `FileTokenizer::build_from_file`
//! walks the installed GGUF's metadata pairs and takes the vocabulary, the
//! scores, the token types, the four special ids and the three `add_*` flags out
//! of the bytes the model ships with. There is no fallback vocabulary: a file
//! that declares no `tokenizer.ggml.tokens` is refused, not guessed at.
//!
//! # WHAT THE INSTALLED FILE ACTUALLY DECLARES
//!
//! Measured against `target/verify/models/embeddinggemma-300M-Q8_0.gguf` by
//! `target/verify/tokenizer_probe.py` (its report is beside it), not assumed:
//!
//! * `tokenizer.ggml.model` = `"llama"`, `tokenizer.ggml.pre` = `"default"`,
//!   `general.architecture` = `"gemma-embedding"`, `general.name` =
//!   `"Embeddinggemma 300m"`.
//! * `tokenizer.ggml.tokens`: string array, 262,144 entries.
//! * `tokenizer.ggml.scores`: f32 array, 262,144 entries, -255504.0 .. 0.0.
//! * `tokenizer.ggml.token_type`: i32 array, 262,144 entries; the histogram is
//!   NORMAL(1) 255,474, CONTROL(3) 6,251, USER_DEFINED(4) 163, BYTE(6) 256.
//! * `bos_token_id` 2, `eos_token_id` 1, `unknown_token_id` 3,
//!   `padding_token_id` 0; `add_bos_token` true, `add_eos_token` true,
//!   `add_space_prefix` **false**.
//! * The 256 byte tokens are named `<0x00>` .. `<0xFF>` and sit at ids
//!   238 ..= 493 -- contiguous, and nowhere near 4. `value == id - 238` for all
//!   256 of them.
//! * ABSENT from this file: `tokenizer.ggml.merges`, `added_tokens`,
//!   `byte_fallback`, `normalize`, `prepend_space`, `n_capture`, `chat_template`.
//!
//! # WHAT `encode` GUARANTEES
//!
//! * Every id it emits is an id this file declared, and every id it emits for a
//!   non-empty span decodes, through `decode_token` over the same table, back to
//!   exactly the bytes it consumed. The host suite asserts that round trip over
//!   hand-written fixtures and over the installed file's own vocabulary.
//! * Longest match wins; where two declared pieces are the same bytes, the one
//!   with the higher `tokenizer.ggml.scores` value wins.
//! * Word boundaries use the file's own `U+2581` marker, consumed against a space
//!   that is really in the input, so no space is invented and none is lost.
//! * The byte fallback comes from the file's `<0xNN>` entries. A file that names
//!   none gets `has_byte_fallback() == false`, and a byte with no piece then goes
//!   to the file's own `unknown_token_id`.
//! * Every bound it enforces refuses with both numbers: `VocabTooLarge(needed,
//!   budget)` for a vocabulary the service will not hold, `VocabHeldTooLarge` for
//!   one that outgrew the estimate, `PieceTooLong` for a piece past the window,
//!   and `BeyondBuffer { needed, bound }` for a text that will not fit the id
//!   buffer it was given.
//!
//! # WHAT `encode` DOES NOT CLAIM -- READ THIS BEFORE CALLING IT A TOKENIZER
//!
//! This is a longest-match encoder over a unigram vocabulary. It is NOT a
//! SentencePiece or llama.cpp-compatible tokenizer, and the gap is measured, not
//! guessed:
//!
//! * A true unigram tokenizer segments a word with Viterbi over the scores,
//!   taking the segmentation with the highest total probability. This takes the
//!   longest piece at each position. On a piece inventory this large the two
//!   agree most of the time and provably disagree some of the time, so nowhere in
//!   this module -- and no assertion in this suite -- claims the ids a reference
//!   runtime would give. None is installed on this host (no sentencepiece, no
//!   llama.cpp, no torch tokenizer), so such a claim could only be fabricated,
//!   which is the exact failure the invented table was.
//! * `tokenizer.ggml.normalize` is absent from this file, so no character
//!   normalisation runs. Input is taken as the bytes it is, and the one input this
//!   module therefore cannot round-trip is text that already contains the marker
//!   character: `\u{2581}` in the request decodes to a space, because that is what
//!   the file says the marker means. Asserted as a limitation, not hidden.
//! * `tokenizer.ggml.pre` = `"default"` is read and reported, and no pre-tokenizer
//!   stage is applied, because `merges`, `added_tokens` and `n_capture` are all
//!   absent and a configurable pre-tokenizer cannot be built from data the file
//!   does not carry. The scan is therefore a raw byte scan, and it says so.
//! * There is no piece cache. Segmentation is recomputed on every call.
//! * `add_space_prefix` is false in this file, and is read, reported and NOT
//!   applied: the rule below consumes a space that exists, which is the half of
//!   SentencePiece's word-prefix behaviour that can be implemented without
//!   inventing bytes the caller did not send.
//! * `add_eos_token` is true in this file, and is read, reported and NOT applied:
//!   this file declares no chat template for the EOS to belong to, and an encoder
//!   that appended a position would add one the caller did not ask for to a run
//!   bounded at 32 positions.
//!
//! WHY the status a reply carries is decided from this table's RESULT rather than
//! from the file's presence: see `TextIds` and `apply_file_vocabulary` in
//! `lib.rs`. A neural label belongs to a run over ids this file produced, and
//! nothing before that point can promise it.

use crate::gguf::{self, GgufError, GgufHeader, GgufStream};
use alloc::vec::Vec;
use core::cmp::Ordering;
use interos_std as std;

/// The four special ids the installed file is measured to declare.
///
/// WHY these constants survive the invented table's removal: they are what this
/// file states for `padding_token_id`/`eos_token_id`/`bos_token_id`/
/// `unknown_token_id`, so a fixture that declares the same numbers lines up with
/// the engine tests that assemble a prompt by hand. A `FileTokenizer` never reads
/// them -- it uses the ids its own file declared, and where those differ it is
/// the file that is right.
pub const TOKEN_PAD: u32 = 0;
/// See `TOKEN_PAD`: this is the installed file's `eos_token_id`, not a default.
pub const TOKEN_EOS: u32 = 1;
/// See `TOKEN_PAD`: this is the installed file's `bos_token_id`, not a default.
pub const TOKEN_BOS: u32 = 2;
/// See `TOKEN_PAD`: this is the installed file's `unknown_token_id`.
pub const TOKEN_UNK: u32 = 3;

/// SentencePiece's word-boundary piece, `U+2581 LOWER ONE EIGHTH BLOCK`.
///
/// WHY it is spelled in bytes: this crate is `#![no_std]` with no Unicode tables,
/// and the file states the marker as these three bytes at the front of 137,512 of
/// its 262,144 pieces.
pub const WORD_MARKER: &[u8] = b"\xe2\x96\x81";

/// Longest piece the table will hold, in bytes.
///
/// WHY a cap: the vocabulary array is the only place in a GGUF file where a
/// length prefix is followed by bytes the reader has to keep, so one huge "token"
/// is a way to ask for memory nothing will hand back. The installed file's
/// longest piece is 48 bytes, so 128 is loose enough for any real Gemma or GPT-2
/// vocabulary and tight enough to be a bound.
pub const MAX_PIECE_BYTES: usize = 128;

/// How many bytes of heap the service will spend holding one file's vocabulary.
///
/// WHY this number: sized against the installed file, whose table the probe
/// measured at 5,672,210 bytes (piece bytes 2,315,366 + start entries 1,048,580 +
/// matchable flags 262,144 + pool 1,022,548 + byte map 1,024). Sixteen mebibytes
/// leaves that file room for the vectors' own growth and still refuses a
/// hundred-megabyte vocabulary on the first read rather than after it.
///
/// WHY a budget at all, when `weights.rs` and `inference.rs` both price first: a
/// vocabulary's byte size is not knowable before it is read -- GGUF states a
/// string array's element count, not its total size -- so the bound is a running
/// charge plus a ceiling at the end, and both refusals name their two numbers.
/// The charge tracks the entries the table ends up holding; the geometric growth
/// of the vectors as they fill is the allocator's and is NOT counted, which is
/// what the headroom above is for. A heap the service cannot grow is a build that
/// aborts, which is why nothing here asks before checking.
pub const TOKENIZER_HEAP_BUDGET: usize = 16 * 1024 * 1024;

/// The per-entry floor charged the moment an array's element count is read,
/// before any element is touched.
///
/// WHY an estimate is charged first: a file that declares four billion tokens has
/// told the reader enough to refuse it, and making the reader walk four billion
/// length prefixes to discover what it already knows is a denial of service
/// written as a parser. Four bytes for the start entry, one for the matchable
/// flag, three for the shortest piece that can carry a word marker.
const MIN_TABLE_BYTES_PER_ENTRY: usize = 8;

/// `tokenizer.ggml.token_type` value for an ordinary piece.
const KIND_NORMAL: i32 = 1;
/// `tokenizer.ggml.token_type` value for a hand-added piece.
const KIND_USER_DEFINED: i32 = 4;

/// The value in `byte_ids` for "this file declares no token for this byte".
const NO_BYTE_ID: u32 = u32::MAX;

/// Why a vocabulary could not be turned into a table.
///
/// WHY the numbers are carried rather than formatted: the service is
/// `#![no_std]` with no formatting machinery on the reply path, and a refusal that
/// says "too large" without the two numbers says nothing a reader can act on.
/// `lib.rs` renders these into the notice, in the style every other refusal here
/// already uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenizerError {
    /// The metadata walk itself refused. Carried whole so the notice can name
    /// which reader failure it was instead of flattening five into one.
    Reader(GgufError),
    /// The file holds no `tokenizer.ggml.tokens` array, or declares only entries
    /// it does not let be emitted. There is no built-in vocabulary to fall back
    /// to, so there is no tokenizer.
    NoVocabulary,
    /// The path the probe read could not be opened when the table was built.
    FileNotFound,
    /// The vocabulary's declared entry count alone needs `needed` bytes against a
    /// budget of `budget`; refused before its elements were read.
    VocabTooLarge(usize, usize),
    /// The live charge during the walk passed `budget`. Carries the charge and the
    /// budget: this is the refusal that fires on a file whose pieces are far
    /// longer than the per-entry floor predicted.
    VocabHeldTooLarge(usize, usize),
    /// Token `id` states `stated` bytes where the window holds `MAX_PIECE_BYTES`.
    PieceTooLong(u32, usize, usize),
    /// `tokenizer.ggml.scores` or `tokenizer.ggml.token_type` declares `declared`
    /// entries beside a vocabulary of `vocab`. Both arrays are indexed by id, so a
    /// short one is a file that does not know its own size.
    ArrayLengthMismatch(&'static str, u32, u32),
    /// An array value declared element type `found` where the key names a fixed
    /// one; the reader will not reinterpret a string array as a float array.
    BadArrayShape(&'static str, u32),
    /// The file stopped inside the named key. Names the key, because "the file is
    /// short" and "the vocabulary is short" are different next moves.
    Truncated(&'static str),
    /// A length or count that does not fit this device's address width.
    ArithmeticOverflow,
}

/// Why a text did not fit the id buffer it was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The whole text was segmented and the segmentation is `needed` positions;
    /// the caller's buffer is `bound`. Nothing was written, because a
    /// half-encoded prompt run through the graph is a vector of the wrong
    /// sentence.
    BeyondBuffer { needed: usize, bound: usize },
}

/// One file's vocabulary, held on the heap and read out of that file.
///
/// WHY a heap table at all, when the rest of the service is zero-allocation: a
/// 262,144-entry vocabulary does not fit on a stack, and both alternatives --
/// re-reading the file per piece, or keeping the invented table -- were measured
/// and refused. The build is one pass over the metadata, the table is dropped when
/// the request is answered, and its size is bounded before it is asked for.
///
/// WHY it is rebuilt per request rather than cached: the `aid` loop is
/// single-threaded and holds no lock it could check a cache against, and the
/// platform's rule is that a service does not take a blocking lock on a request
/// path. Caching would mean either a lock or an assumption about which file is
/// installed; both are worse than the walk, whose cost the real-file test prints.
pub struct FileTokenizer {
    /// Every declared piece's bytes, concatenated, in id order.
    bytes: Vec<u8>,
    /// `starts[id]` is the offset of id's bytes in `bytes` and
    /// `starts[vocab]` is `bytes.len()`, so a piece's length is two reads.
    starts: Vec<u32>,
    /// Whether an id may be emitted by `encode`: the file's own `token_type` says
    /// so, and the id is neither one of the specials the file named nor one of its
    /// byte tokens.
    matchable: Vec<bool>,
    /// The matchable ids, sorted by piece bytes and then by descending score, so a
    /// binary search finds a piece and the first entry carrying those bytes is the
    /// better-scoring one.
    ///
    /// WHY a sorted pool and not a first-byte bucket: measured on this file,
    /// 138,724 pieces begin with the marker's first byte `0xE2`, so a 256-way
    /// bucket collapses to one bucket holding half the vocabulary. A sort costs
    /// once per build; every probe is then `log n`.
    pool: Vec<u32>,
    /// `byte_ids[byte]` is the id of the `<0xNN>` token this file declared, or
    /// `NO_BYTE_ID`. Derived from the file's own names, never from an offset.
    byte_ids: [u32; 256],
    /// How many bytes this file covers; `0` means it has no byte tokens at all.
    byte_width: usize,
    /// The lowest id the file uses for a byte token -- 238 for the installed file,
    /// where the invented table said 4.
    byte_first_id: u32,
    /// Longest matchable piece, so the scan never probes past what exists.
    max_piece_len: usize,
    /// Whether any matchable piece begins with the word marker.
    marker_pieces: bool,
    /// What the vocabulary array declared, counted from its own header.
    declared_vocab: u32,
    bos_id: Option<u32>,
    eos_id: Option<u32>,
    pad_id: Option<u32>,
    unk_id: Option<u32>,
    add_bos_token: Option<bool>,
    add_eos_token: Option<bool>,
    add_space_prefix: Option<bool>,
    byte_fallback_flag: Option<bool>,
    scores_declared: bool,
    types_declared: bool,
    /// What the build charged against `TOKENIZER_HEAP_BUDGET`.
    heap_bytes: usize,
}

impl FileTokenizer {
    /// Reads the installed file's metadata and builds its vocabulary table.
    ///
    /// WHY the descriptor is opened and closed here: the walk needs the whole
    /// metadata block and nothing else, and a caller that held the descriptor would
    /// be holding it across a six-megabyte read for no reason. The window belongs
    /// to the stream and goes with it.
    pub fn build_from_file(path: &str) -> Result<Self, TokenizerError> {
        let fd = std::fs_open(path, std::FS_READ).map_err(|_| TokenizerError::FileNotFound)?;
        let built = {
            let mut stream = GgufStream::buffered_from_file(fd);
            Self::build_from_stream(&mut stream)
        };
        let _ = std::fs_close(fd);
        built
    }

    /// Builds the table from any GGUF stream, so the walk that runs on the device
    /// can also be driven from a fixture and from a slice of the real file.
    pub fn build_from_stream(stream: &mut GgufStream<'_>) -> Result<Self, TokenizerError> {
        let header = GgufHeader::parse_stream(stream).map_err(TokenizerError::Reader)?;
        let mut read = VocabularyRead::new();

        for _ in 0..header.kv_count {
            let key_len = stream
                .read_u64()
                .map_err(|error| at("a metadata key", error))?;
            let key_len =
                usize::try_from(key_len).map_err(|_| TokenizerError::ArithmeticOverflow)?;
            if key_len == 0 || key_len > gguf::MAX_METADATA_KEY_LEN {
                // Not a key this reader names: step over the name and its value.
                stream
                    .skip(key_len)
                    .map_err(|error| at("a metadata key", error))?;
                let val_type = stream
                    .read_u32()
                    .map_err(|error| at("a metadata key", error))?;
                gguf::skip_metadata_value_stream(stream, val_type)
                    .map_err(|error| at("an unrecognised metadata value", error))?;
                continue;
            }
            let mut key_buf = [0u8; gguf::MAX_METADATA_KEY_LEN];
            stream
                .read_exact(&mut key_buf[..key_len])
                .map_err(|error| at("a metadata key", error))?;
            let val_type = stream
                .read_u32()
                .map_err(|error| at("a metadata value type", error))?;
            let key = &key_buf[..key_len];

            // WHY whole keys and not suffixes: `scores` beside `tokenizer.ggml`
            // and `scores` beside `bert` are not the same array, and a walk that
            // matched the tail of a key would load another family's scores and use
            // them to rank this file's pieces.
            if key == b"tokenizer.ggml.tokens" {
                read.tokens(stream, val_type)?;
            } else if key == b"tokenizer.ggml.scores" {
                read.scores(stream, val_type)?;
            } else if key == b"tokenizer.ggml.token_type" {
                read.kinds(stream, val_type)?;
            } else if key == b"tokenizer.ggml.bos_token_id" {
                let id = read.scalar_id(stream, val_type, "tokenizer.ggml.bos_token_id")?;
                read.bos_id = Some(id);
            } else if key == b"tokenizer.ggml.eos_token_id" {
                let id = read.scalar_id(stream, val_type, "tokenizer.ggml.eos_token_id")?;
                read.eos_id = Some(id);
            } else if key == b"tokenizer.ggml.padding_token_id" {
                let id = read.scalar_id(stream, val_type, "tokenizer.ggml.padding_token_id")?;
                read.pad_id = Some(id);
            } else if key == b"tokenizer.ggml.unknown_token_id" {
                let id = read.scalar_id(stream, val_type, "tokenizer.ggml.unknown_token_id")?;
                read.unk_id = Some(id);
            } else if key == b"tokenizer.ggml.add_bos_token" {
                let flag = read.flag(stream, val_type, "tokenizer.ggml.add_bos_token")?;
                read.add_bos_token = Some(flag);
            } else if key == b"tokenizer.ggml.add_eos_token" {
                let flag = read.flag(stream, val_type, "tokenizer.ggml.add_eos_token")?;
                read.add_eos_token = Some(flag);
            } else if key == b"tokenizer.ggml.add_space_prefix" {
                let flag = read.flag(stream, val_type, "tokenizer.ggml.add_space_prefix")?;
                read.add_space_prefix = Some(flag);
            } else if key == b"tokenizer.ggml.byte_fallback" {
                // Read and reported, never needed: the byte tokens above are
                // derived from the file's own `<0xNN>` names, which is stronger
                // evidence than a flag. A file with the names and not the flag
                // still gets a fallback; a file with the flag and not the names
                // gets none, and `has_byte_fallback` says so.
                let flag = read.flag(stream, val_type, "tokenizer.ggml.byte_fallback")?;
                read.byte_fallback_flag = Some(flag);
            } else {
                gguf::skip_metadata_value_stream(stream, val_type)
                    .map_err(|error| at("an unrecognised metadata value", error))?;
            }
        }

        read.finish()
    }

    /// Segments `text` into this file's own ids.
    ///
    /// The rule, stated exactly so the header above can be checked against the
    /// code: at each byte position, when that position holds the first space of its
    /// run, try the longest piece whose bytes are the marker followed by a prefix
    /// of what comes after the space, and let it consume the space as well as its
    /// own remainder. Otherwise try the longest piece that is a prefix of the input
    /// at this position. If neither exists, emit this file's `<0xNN>` id for the
    /// byte; if the file declares no byte tokens, emit its `unknown_token_id`; if
    /// it declares neither, emit nothing for that byte and move on, which is the
    /// only honest answer for a file with no way to say what a byte is.
    ///
    /// WHY the space is consumed rather than invented: a piece that begins with the
    /// marker decodes to a space, so a match that consumed a real space round-trips
    /// exactly, and a match that invented one would put a space in the text the
    /// caller never sent. This is what makes `decode_sequence(encode(t)) == t` hold
    /// for every input the file can name, which the suite asserts.
    ///
    /// Returns the number of positions written. A text whose segmentation is longer
    /// than `out` is refused with both numbers and writes nothing, because a
    /// truncated prompt is a different prompt.
    pub fn encode(&self, text: &[u8], out: &mut [u32]) -> Result<usize, EncodeError> {
        // Pass one counts the segmentation, pass two writes it. WHY two walks over
        // the text instead of writing while counting: the number of positions is
        // only known when the last one is matched, and a refusal that had already
        // left the opening ids in the caller's buffer would hand a `let _ =` caller
        // the beginning of a prompt it was told was refused. There is nothing to
        // write back, either -- the buffer's own prior bytes were never promised
        // to be zero.
        let needed = self.segment(text, |_| {});
        if needed > out.len() {
            return Err(EncodeError::BeyondBuffer {
                needed,
                bound: out.len(),
            });
        }
        let mut written = 0usize;
        self.segment(text, |id| {
            out[written] = id;
            written += 1;
        });
        Ok(written)
    }

    /// One greedy walk over `text`, handing every id the segmentation produces to
    /// `emit` and returning how many there were.
    ///
    /// WHY the walk is behind one function: `encode` has to see the same
    /// segmentation twice, and two copies of the matching rules is how a count and
    /// a write would drift apart and make the bound above mean nothing.
    fn segment(&self, text: &[u8], mut emit: impl FnMut(u32)) -> usize {
        let mut written = 0usize;

        // The file's own `add_bos_token`, and only if the file also named the id.
        if self.writes_bos() {
            written += 1;
            emit(self.bos_id.expect("writes_bos checked the id"));
        }

        let mut position = 0usize;
        while position < text.len() {
            let word_start = position == 0 || text[position - 1] != b' ';
            let mut consumed = 0usize;
            let mut emitted: Option<u32> = None;

            // The word-boundary branch: only where a real space stands at the front
            // of a word, and only for a file whose pieces carry the marker.
            if word_start
                && text[position] == b' '
                && self.marker_pieces
                && self.max_piece_len > WORD_MARKER.len()
            {
                let take = (text.len() - position - 1).min(self.max_piece_len - WORD_MARKER.len());
                let mut needle = [0u8; MAX_PIECE_BYTES];
                needle[..WORD_MARKER.len()].copy_from_slice(WORD_MARKER);
                needle[WORD_MARKER.len()..WORD_MARKER.len() + take]
                    .copy_from_slice(&text[position + 1..position + 1 + take]);
                let full = WORD_MARKER.len() + take;
                for probe in (WORD_MARKER.len()..=full).rev() {
                    if let Some(id) = self.find_best(&needle[..probe]) {
                        emitted = Some(id);
                        // The piece is the marker plus a remainder; the input it
                        // stands for is the space plus that same remainder.
                        consumed = 1 + self.bytes_of(id).len() - WORD_MARKER.len();
                        break;
                    }
                }
            }

            if emitted.is_none() {
                let limit = (text.len() - position).min(self.max_piece_len);
                for length in (1..=limit).rev() {
                    if let Some(id) = self.find_best(&text[position..position + length]) {
                        emitted = Some(id);
                        consumed = length;
                        break;
                    }
                }
            }

            match emitted {
                Some(id) => {
                    written += 1;
                    emit(id);
                    position += consumed;
                }
                None => {
                    if let Some(id) = self.byte_id_of(text[position]).or(self.unk_id) {
                        written += 1;
                        emit(id);
                    }
                    position += 1;
                }
            }
        }

        written
    }

    /// The bytes one id stands for, with the word marker rendered as a space and a
    /// byte token rendered as the byte it names.
    ///
    /// WHY byte tokens are answered before the piece table: the installed file's id
    /// 270 is named `<0x20>`, and copying those six literal characters out of the
    /// vocabulary would put `<0x20>` in the decoded text where a space belongs. A
    /// probe of the round trip found exactly that, which is why this order is
    /// pinned here and asserted in the suite.
    pub fn decode_token(&self, id: u32, out: &mut [u8]) -> usize {
        if out.is_empty() || self.is_special(id) {
            return 0;
        }
        if let Some(byte) = self.byte_of_id(id) {
            out[0] = byte;
            return 1;
        }
        if id >= self.declared_vocab {
            return 0;
        }
        let mut written = 0usize;
        let mut rest = self.bytes_of(id);
        while !rest.is_empty() && written < out.len() {
            if rest.starts_with(WORD_MARKER) {
                out[written] = b' ';
                written += 1;
                rest = &rest[WORD_MARKER.len()..];
            } else {
                out[written] = rest[0];
                written += 1;
                rest = &rest[1..];
            }
        }
        written
    }

    /// Decodes a whole id stream. Stops at the end of `out` and reports how many
    /// bytes it wrote, so a caller can tell a full buffer from a short text.
    pub fn decode_sequence(&self, ids: &[u32], out: &mut [u8]) -> usize {
        let mut total = 0usize;
        for &id in ids {
            if total >= out.len() {
                break;
            }
            total += self.decode_token(id, &mut out[total..]);
        }
        total
    }

    // -- accessors: what the file declared, reported rather than assumed. -------

    /// Entries the vocabulary array declared.
    pub fn declared_vocab(&self) -> u32 {
        self.declared_vocab
    }
    /// Pieces the encoder is allowed to emit.
    pub fn matchable_pieces(&self) -> usize {
        self.pool.len()
    }
    /// Whether the file lets `id` be emitted by `encode` at all: its own
    /// `token_type` says it is a piece, and it is neither a named special nor one
    /// of its `<0xNN>` byte tokens.
    ///
    /// WHY this is kept rather than folded into `pool`: `pool` answers "what bytes
    /// are there" and is ordered by bytes, so it cannot answer a question asked of
    /// an id. The flags are the only per-id view of the file's own rule, which is
    /// why `TOKENIZER_HEAP_BUDGET` counts them in the shipped table instead of
    /// charging and releasing them like the scores and types.
    pub fn may_emit(&self, id: u32) -> bool {
        self.matchable.get(id as usize).copied().unwrap_or(false)
    }
    /// How many bytes this file covers with `<0xNN>` tokens; `0` when none.
    pub fn byte_fallback_width(&self) -> usize {
        self.byte_width
    }
    /// The lowest id the file uses for a byte token.
    pub fn byte_fallback_first_id(&self) -> Option<u32> {
        (self.byte_width > 0).then_some(self.byte_first_id)
    }
    /// Whether the file gave the encoder a way to say what a byte is.
    pub fn has_byte_fallback(&self) -> bool {
        self.byte_width > 0
    }
    pub fn bos_id(&self) -> Option<u32> {
        self.bos_id
    }
    pub fn eos_id(&self) -> Option<u32> {
        self.eos_id
    }
    pub fn pad_id(&self) -> Option<u32> {
        self.pad_id
    }
    pub fn unk_id(&self) -> Option<u32> {
        self.unk_id
    }
    /// `tokenizer.ggml.add_bos_token`, as the file stated it.
    pub fn adds_bos_token(&self) -> Option<bool> {
        self.add_bos_token
    }
    /// `tokenizer.ggml.add_eos_token`. Read and reported; NOT applied -- see the
    /// module header for why appending a position would be wrong.
    pub fn adds_eos_token(&self) -> Option<bool> {
        self.add_eos_token
    }
    /// `tokenizer.ggml.add_space_prefix`. Read and reported; NOT applied.
    pub fn adds_space_prefix(&self) -> Option<bool> {
        self.add_space_prefix
    }
    /// `tokenizer.ggml.byte_fallback`, which this file does not carry at all.
    pub fn declares_byte_fallback_flag(&self) -> Option<bool> {
        self.byte_fallback_flag
    }
    /// Whether the file's pieces carry the word marker at all.
    pub fn encodes_word_boundaries(&self) -> bool {
        self.marker_pieces
    }
    /// Whether `tokenizer.ggml.scores` was present. Without it, two pieces with the
    /// same bytes are ranked by id instead.
    pub fn declares_scores(&self) -> bool {
        self.scores_declared
    }
    /// Whether `tokenizer.ggml.token_type` was present.
    pub fn declares_token_types(&self) -> bool {
        self.types_declared
    }
    /// Longest piece the encoder will try to match.
    pub fn max_piece_len(&self) -> usize {
        self.max_piece_len
    }
    /// What the build charged against `TOKENIZER_HEAP_BUDGET`.
    pub fn heap_bytes(&self) -> usize {
        self.heap_bytes
    }

    /// Whether `encode` opens with the file's bos id, from the file's own flag.
    fn writes_bos(&self) -> bool {
        matches!(self.add_bos_token, Some(true)) && self.bos_id.is_some()
    }

    fn bytes_of(&self, id: u32) -> &[u8] {
        let index = id as usize;
        &self.bytes[self.starts[index] as usize..self.starts[index + 1] as usize]
    }

    fn is_special(&self, id: u32) -> bool {
        self.bos_id == Some(id)
            || self.eos_id == Some(id)
            || self.pad_id == Some(id)
            || self.unk_id == Some(id)
    }

    /// The id this file declares for `byte`, if any.
    pub fn byte_id_of(&self, byte: u8) -> Option<u32> {
        let id = self.byte_ids[byte as usize];
        (id != NO_BYTE_ID).then_some(id)
    }

    /// The byte an id names, when the id is one of this file's byte tokens.
    pub fn byte_of_id(&self, id: u32) -> Option<u8> {
        if self.byte_width == 0 {
            return None;
        }
        // WHY a scan and not arithmetic on the first id: the installed file's 256
        // byte tokens are contiguous (238..=493), but that is a property of this
        // file -- measured, reported, and nowhere promised by the format. A scan of
        // 256 entries reads the file's own names either way.
        (0u32..256)
            .find(|&byte| self.byte_ids[byte as usize] == id)
            .map(|byte| byte as u8)
    }

    /// The highest-scoring declared piece that is exactly `needle`.
    fn find_best(&self, needle: &[u8]) -> Option<u32> {
        // Lower bound over a pool sorted by piece bytes: the first entry whose
        // bytes are not less than `needle` is either `needle` with the best score
        // attached to it, or something else -- which means `needle` is absent.
        let mut low = 0usize;
        let mut high = self.pool.len();
        while low < high {
            let mid = low + (high - low) / 2;
            if self.bytes_of(self.pool[mid]).cmp(needle) == Ordering::Less {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let id = *self.pool.get(low)?;
        (self.bytes_of(id) == needle).then_some(id)
    }
}

/// A truncation inside a named key is reported as the file stopping, whatever the
/// reader's own error word was.
///
/// WHY a free function: the walk calls it while holding a mutable borrow of the
/// builder it is filling, and a method on that builder would be a second borrow of
/// the thing being filled.
fn at(key: &'static str, error: GgufError) -> TokenizerError {
    match error {
        GgufError::BufferTooShort | GgufError::ReadFailed => TokenizerError::Truncated(key),
        other => TokenizerError::Reader(other),
    }
}

/// The bytes of one id's piece, from the two vectors a build is mid-way through.
fn piece_bytes<'b>(bytes: &'b [u8], starts: &[u32], id: u32) -> &'b [u8] {
    let low = starts[id as usize] as usize;
    let high = starts[id as usize + 1] as usize;
    &bytes[low..high]
}

/// The byte value a `<0xNN>` name states, or `None` for any other name.
fn byte_token_value(name: &[u8]) -> Option<u8> {
    if name.len() != 6 || &name[..3] != b"<0x" || name[5] != b'>' {
        return None;
    }
    let high = hex_digit(name[3])?;
    let low = hex_digit(name[4])?;
    Some(high << 4 | low)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One metadata block's worth of vocabulary, while it is still being read.
///
/// WHY the scores and the types are live here and gone in `FileTokenizer`: the only
/// things they are used for are choosing between two pieces with the same bytes and
/// deciding which pieces may be emitted at all. Both are answered during the build,
/// and carrying 1.3 MB of numbers the encoder never looks at again would be paying
/// for a fact after spending it.
struct VocabularyRead {
    bytes: Vec<u8>,
    starts: Vec<u32>,
    kinds: Vec<i32>,
    scores: Vec<f32>,
    byte_ids: [u32; 256],
    byte_width: usize,
    byte_first_id: u32,
    vocab: Option<u32>,
    held: usize,
    bos_id: Option<u32>,
    eos_id: Option<u32>,
    pad_id: Option<u32>,
    unk_id: Option<u32>,
    add_bos_token: Option<bool>,
    add_eos_token: Option<bool>,
    add_space_prefix: Option<bool>,
    byte_fallback_flag: Option<bool>,
    scores_declared: bool,
    types_declared: bool,
}

impl VocabularyRead {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            starts: Vec::new(),
            kinds: Vec::new(),
            scores: Vec::new(),
            byte_ids: [NO_BYTE_ID; 256],
            byte_width: 0,
            byte_first_id: NO_BYTE_ID,
            vocab: None,
            // The byte map is a kibibyte and exists before any key is read.
            held: core::mem::size_of::<[u32; 256]>(),
            bos_id: None,
            eos_id: None,
            pad_id: None,
            unk_id: None,
            add_bos_token: None,
            add_eos_token: None,
            add_space_prefix: None,
            byte_fallback_flag: None,
            scores_declared: false,
            types_declared: false,
        }
    }

    /// Charges `amount` against the budget and refuses the build past it.
    fn charge(&mut self, amount: usize) -> Result<(), TokenizerError> {
        let next = self
            .held
            .checked_add(amount)
            .ok_or(TokenizerError::ArithmeticOverflow)?;
        if next > TOKENIZER_HEAP_BUDGET {
            return Err(TokenizerError::VocabHeldTooLarge(next, TOKENIZER_HEAP_BUDGET));
        }
        self.held = next;
        Ok(())
    }

    /// `tokenizer.ggml.tokens`: a string array, read element by element.
    fn tokens(
        &mut self,
        stream: &mut GgufStream<'_>,
        val_type: u32,
    ) -> Result<(), TokenizerError> {
        if val_type != 9 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.tokens",
                val_type,
            ));
        }
        let elem_type = stream
            .read_u32()
            .map_err(|error| at("tokenizer.ggml.tokens", error))?;
        if elem_type != 8 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.tokens element type",
                elem_type,
            ));
        }
        let declared = stream
            .read_u64()
            .map_err(|error| at("tokenizer.ggml.tokens", error))?;
        let Ok(declared) = u32::try_from(declared) else {
            return Err(TokenizerError::VocabTooLarge(
                usize::MAX,
                TOKENIZER_HEAP_BUDGET,
            ));
        };
        // The floor charge, before a single element is touched; see
        // `MIN_TABLE_BYTES_PER_ENTRY` for why a count alone is enough to refuse.
        let projected = (declared as usize).saturating_mul(MIN_TABLE_BYTES_PER_ENTRY);
        if projected > TOKENIZER_HEAP_BUDGET {
            return Err(TokenizerError::VocabTooLarge(
                projected,
                TOKENIZER_HEAP_BUDGET,
            ));
        }
        self.vocab = Some(declared);
        self.starts.push(0);

        for id in 0..declared {
            let len = stream
                .read_u64()
                .map_err(|error| at("tokenizer.ggml.tokens", error))?;
            let len = usize::try_from(len).map_err(|_| TokenizerError::ArithmeticOverflow)?;
            if len > MAX_PIECE_BYTES {
                return Err(TokenizerError::PieceTooLong(id, len, MAX_PIECE_BYTES));
            }
            self.charge(len)?;
            let mut piece = [0u8; MAX_PIECE_BYTES];
            stream
                .read_exact(&mut piece[..len])
                .map_err(|error| at("tokenizer.ggml.tokens", error))?;
            self.bytes.extend_from_slice(&piece[..len]);
            let next = u32::try_from(self.bytes.len()).map_err(|_| TokenizerError::ArithmeticOverflow)?;
            self.starts.push(next);

            // The byte map is built from the file's own names as they pass, which
            // is the only reason this walk needs the strings rather than their
            // lengths. The six name bytes go to the stack before the map is written,
            // because the map and the piece are two borrows of the same step.
            let mut name = [0u8; 6];
            if len == 6 {
                name.copy_from_slice(&piece[..6]);
            }
            if let Some(byte) = byte_token_value(&name) {
                if self.byte_width == 0 {
                    self.byte_first_id = id;
                }
                self.byte_width += 1;
                self.byte_ids[byte as usize] = id;
            }
        }
        Ok(())
    }

    /// `tokenizer.ggml.scores`: the unigram log-probabilities, used only to break a
    /// tie between two pieces with identical bytes.
    fn scores(
        &mut self,
        stream: &mut GgufStream<'_>,
        val_type: u32,
    ) -> Result<(), TokenizerError> {
        if val_type != 9 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.scores",
                val_type,
            ));
        }
        let elem_type = stream
            .read_u32()
            .map_err(|error| at("tokenizer.ggml.scores", error))?;
        if elem_type != 6 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.scores element type",
                elem_type,
            ));
        }
        let count = self.count_of(stream, "tokenizer.ggml.scores")?;
        for _ in 0..count {
            self.charge(4)?;
            let bits = stream
                .read_u32()
                .map_err(|error| at("tokenizer.ggml.scores", error))?;
            self.scores.push(f32::from_bits(bits));
        }
        self.scores_declared = true;
        Ok(())
    }

    /// `tokenizer.ggml.token_type`: what each entry is, which is what decides
    /// whether an entry is a piece at all or only a byte token.
    fn kinds(&mut self, stream: &mut GgufStream<'_>, val_type: u32) -> Result<(), TokenizerError> {
        if val_type != 9 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.token_type",
                val_type,
            ));
        }
        let elem_type = stream
            .read_u32()
            .map_err(|error| at("tokenizer.ggml.token_type", error))?;
        if elem_type != 5 {
            return Err(TokenizerError::BadArrayShape(
                "tokenizer.ggml.token_type element type",
                elem_type,
            ));
        }
        let count = self.count_of(stream, "tokenizer.ggml.token_type")?;
        for _ in 0..count {
            self.charge(4)?;
            let bits = stream
                .read_u32()
                .map_err(|error| at("tokenizer.ggml.token_type", error))?;
            self.kinds.push(bits as i32);
        }
        self.types_declared = true;
        Ok(())
    }

    fn count_of(
        &self,
        stream: &mut GgufStream<'_>,
        key: &'static str,
    ) -> Result<u32, TokenizerError> {
        let count = stream.read_u64().map_err(|error| at(key, error))?;
        u32::try_from(count)
            .map_err(|_| TokenizerError::VocabTooLarge(usize::MAX, TOKENIZER_HEAP_BUDGET))
    }

    /// A scalar id, whatever width the file declared it in -- GGUF writers differ.
    fn scalar_id(
        &mut self,
        stream: &mut GgufStream<'_>,
        val_type: u32,
        key: &'static str,
    ) -> Result<u32, TokenizerError> {
        let wide: u64 = match val_type {
            0 | 1 | 7 => {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).map_err(|error| at(key, error))?;
                byte[0] as u64
            }
            2 | 3 => {
                let mut bytes = [0u8; 2];
                stream.read_exact(&mut bytes).map_err(|error| at(key, error))?;
                u16::from_le_bytes(bytes) as u64
            }
            4 | 5 => stream.read_u32().map_err(|error| at(key, error))? as u64,
            10 | 11 => stream.read_u64().map_err(|error| at(key, error))?,
            _ => return Err(TokenizerError::Reader(GgufError::InvalidMetadata)),
        };
        u32::try_from(wide).map_err(|_| TokenizerError::ArithmeticOverflow)
    }

    /// A type-7 flag: one byte wide, and only `0` or `1` answers it.
    fn flag(
        &mut self,
        stream: &mut GgufStream<'_>,
        val_type: u32,
        key: &'static str,
    ) -> Result<bool, TokenizerError> {
        if val_type != 7 {
            return Err(TokenizerError::Reader(GgufError::InvalidMetadata));
        }
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).map_err(|error| at(key, error))?;
        match byte[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(TokenizerError::Reader(GgufError::InvalidMetadata)),
        }
    }

    /// Turns what was read into the table, or refuses.
    ///
    /// WHY it destructures `self`: the sort below borrows `bytes` and `starts` for
    /// the life of its comparator, and `scores` and `kinds` are read off the same
    /// struct on the way past it. Taking the fields into locals first is how the
    /// borrow checker is told that the two facts are spent here.
    fn finish(self) -> Result<FileTokenizer, TokenizerError> {
        let Self {
            bytes,
            starts,
            kinds,
            scores,
            byte_ids,
            byte_width,
            byte_first_id,
            vocab,
            held,
            bos_id,
            eos_id,
            pad_id,
            unk_id,
            add_bos_token,
            add_eos_token,
            add_space_prefix,
            byte_fallback_flag,
            scores_declared,
            types_declared,
        } = self;

        let vocab = vocab.ok_or(TokenizerError::NoVocabulary)?;
        if starts.len() != vocab as usize + 1 {
            return Err(TokenizerError::ArrayLengthMismatch(
                "tokenizer.ggml.tokens",
                starts.len().saturating_sub(1) as u32,
                vocab,
            ));
        }
        if scores_declared && scores.len() as u32 != vocab {
            return Err(TokenizerError::ArrayLengthMismatch(
                "tokenizer.ggml.scores",
                scores.len() as u32,
                vocab,
            ));
        }
        if types_declared && kinds.len() as u32 != vocab {
            return Err(TokenizerError::ArrayLengthMismatch(
                "tokenizer.ggml.token_type",
                kinds.len() as u32,
                vocab,
            ));
        }

        // Which ids the encoder may emit. Where the file has types, the file says
        // so: NORMAL and USER_DEFINED are pieces, CONTROL and BYTE are not. A file
        // with no types gets the weaker rule -- not one of the named specials and
        // not a `<0xNN>` byte name -- and `declares_token_types()` reports which of
        // the two was used, because they are not the same claim.
        let mut held = held;
        let mut matchable = Vec::with_capacity(vocab as usize);
        let mut pool: Vec<u32> = Vec::new();
        let mut max_piece_len = 0usize;
        let mut marker_pieces = false;
        for id in 0..vocab {
            let allowed = if types_declared {
                let kind = kinds[id as usize];
                kind == KIND_NORMAL || kind == KIND_USER_DEFINED
            } else {
                true
            };
            let named_special = Some(id) == bos_id
                || Some(id) == eos_id
                || Some(id) == pad_id
                || Some(id) == unk_id;
            // A byte token is never a matchable piece. Measured on this file, every
            // `<0xNN>` entry carries score 0.0 -- the highest value in the array --
            // so a rule that ranked bytes alongside the real pieces would prefer one
            // byte over any piece it overlapped. They are fallback, not rank.
            let is_byte = byte_token_value(piece_bytes(&bytes, &starts, id)).is_some();
            let emit = allowed && !named_special && !is_byte;
            // One start entry and one matchable flag per id, whether or not the id
            // is ever emitted: the table is indexed by id, so it is sized by id.
            held = held.saturating_add(4 + 1);
            matchable.push(emit);
            if emit {
                let piece = piece_bytes(&bytes, &starts, id);
                if piece.len() > max_piece_len {
                    max_piece_len = piece.len();
                }
                if piece.starts_with(WORD_MARKER) {
                    marker_pieces = true;
                }
                pool.push(id);
            }
        }
        held = held.saturating_add(pool.len() * core::mem::size_of::<u32>());

        if scores_declared {
            pool.sort_unstable_by(|a, b| {
                piece_bytes(&bytes, &starts, *a)
                    .cmp(piece_bytes(&bytes, &starts, *b))
                    .then_with(|| {
                        let left = score_of(&scores, *a);
                        let right = score_of(&scores, *b);
                        right.total_cmp(&left).then_with(|| a.cmp(b))
                    })
            });
        } else {
            // The id is the tie-break with no scores to rank by, and naming it here
            // is what makes the order total: `sort_unstable` over two pieces that
            // compare equal would otherwise put either of them first.
            pool.sort_unstable_by(|a, b| {
                piece_bytes(&bytes, &starts, *a)
                    .cmp(piece_bytes(&bytes, &starts, *b))
                    .then_with(|| a.cmp(b))
            });
        }

        // The scores and the types have now been spent: on the tie-break above and
        // on the matchable flags. Hand their charge back before the ceiling is
        // measured, so what is refused is the table that ships and not the
        // scaffolding that built it. The spare capacity each vector grew into was
        // never charged, so it is never released either.
        let transient =
            scores.len() * core::mem::size_of::<f32>() + kinds.len() * core::mem::size_of::<i32>();
        held = held.saturating_sub(transient);
        drop(scores);
        drop(kinds);

        if held > TOKENIZER_HEAP_BUDGET {
            return Err(TokenizerError::VocabHeldTooLarge(held, TOKENIZER_HEAP_BUDGET));
        }
        // A vocabulary the encoder cannot match against is a vocabulary it cannot
        // encode with; that is the same gap as having no vocabulary at all.
        if pool.is_empty() {
            return Err(TokenizerError::NoVocabulary);
        }

        Ok(FileTokenizer {
            bytes,
            starts,
            matchable,
            pool,
            byte_ids,
            byte_width,
            byte_first_id,
            max_piece_len,
            marker_pieces,
            declared_vocab: vocab,
            bos_id,
            eos_id,
            pad_id,
            unk_id,
            add_bos_token,
            add_eos_token,
            add_space_prefix,
            byte_fallback_flag,
            scores_declared,
            types_declared,
            heap_bytes: held,
        })
    }
}

/// The score a file declared for an id, or the lowest a rank can hold for an id the
/// array did not reach.
fn score_of(scores: &[f32], id: u32) -> f32 {
    scores.get(id as usize).copied().unwrap_or(f32::MIN)
}

/// The fixture vocabulary below, reachable from another module's tests.
///
/// WHY this bridge exists: the byte-correct GGUF that has the installed file's
/// shape is written once, here. A second copy in `lib.rs` is how a fixture drifts
/// away from the shape this reader is careful about -- the same reason `gguf.rs`
/// re-exports its writers instead of duplicating them.
#[cfg(test)]
pub(crate) mod test_vocabulary {
    pub(crate) use super::tests::installed_shape;
}

#[cfg(test)]
mod tests {
    extern crate std as host;
    use super::*;
    use crate::gguf::test_writers as tw;
    use alloc::vec;
    /// `<0x00>` .. `<0xFF>`, in that order: the shape the installed file's byte
    /// block has, so the fixture's byte ids are derived by the same code path the
    /// real file goes through instead of being typed in.
    const fn byte_names() -> [[u8; 6]; 256] {
        let digits = b"0123456789abcdef";
        let mut out = [[0u8; 6]; 256];
        let mut value = 0usize;
        while value < 256 {
            out[value][0] = b'<';
            out[value][1] = b'0';
            out[value][2] = b'x';
            out[value][3] = digits[value / 16];
            out[value][4] = digits[value % 16];
            out[value][5] = b'>';
            value += 1;
        }
        out
    }

    static BYTE_NAMES: [[u8; 6]; 256] = byte_names();

    /// Five control tokens, the bare word marker, four word-boundary pieces and
    /// four plain pieces -- then the byte block, so this fixture's ids 13..=268
    /// mean exactly what the installed file's 238..=493 mean.
    const HEAD: &[&[u8]] = &[
        b"<pad>",
        b"<eos>",
        b"<bos>",
        b"<unk>",
        b"<mask>",
        "\u{2581}".as_bytes(),
        "\u{2581}battery".as_bytes(),
        "\u{2581}status".as_bytes(),
        "\u{2581}is".as_bytes(),
        b"battery",
        b"status",
        b"ate",
        b"a",
    ];

    const HEAD_KINDS: &[i32] = &[3, 3, 3, 3, 3, 1, 1, 1, 1, 1, 1, 1, 1];

    /// The id the fixture's first byte token sits at, and the id a byte maps to --
    /// derived arithmetic, so no expectation below restates a number the code could
    /// have invented.
    const BYTE_BASE: u32 = 13;

    fn byte_id(value: u8) -> u32 {
        BYTE_BASE + value as u32
    }

    fn vocabulary() -> Vec<&'static [u8]> {
        let mut tokens: Vec<&'static [u8]> = Vec::with_capacity(HEAD.len() + 256);
        tokens.extend_from_slice(HEAD);
        for name in BYTE_NAMES.iter() {
            tokens.push(name);
        }
        tokens
    }

    fn types() -> Vec<i32> {
        let mut kinds: Vec<i32> = HEAD_KINDS.to_vec();
        kinds.extend(core::iter::repeat(6).take(256));
        kinds
    }

    /// A GGUF fixture builder, so a test declares a file's keys in the words the
    /// file uses rather than at byte offsets.
    struct Fixture {
        bytes: Vec<u8>,
        off: usize,
        pairs: u64,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                bytes: vec![0u8; 64],
                off: GgufHeader::HEADER_SIZE,
                pairs: 0,
            }
        }

        fn room(&mut self, extra: usize) {
            if self.off + extra >= self.bytes.len() {
                self.bytes.resize(self.off + extra + 256, 0);
            }
        }

        fn string(&mut self, key: &[u8], value: &[u8]) -> &mut Self {
            self.room(key.len() + value.len() + 24);
            tw::put_string(&mut self.bytes, &mut self.off, key, value);
            self.pairs += 1;
            self
        }

        fn id(&mut self, key: &[u8], value: u32) -> &mut Self {
            self.room(key.len() + 24);
            tw::put_count(&mut self.bytes, &mut self.off, key, value);
            self.pairs += 1;
            self
        }

        fn flag(&mut self, key: &[u8], value: bool) -> &mut Self {
            self.room(key.len() + 16);
            tw::put_bool(&mut self.bytes, &mut self.off, key, value);
            self.pairs += 1;
            self
        }

        /// The named key, declared type 7, and the raw byte the file states -- which
        /// is how a flag that is neither 0 nor 1 gets written.
        fn raw_flag(&mut self, key: &[u8], byte: u8) -> &mut Self {
            self.room(key.len() + 16);
            tw::put_key(&mut self.bytes, &mut self.off, key);
            tw::put_u32_at(&mut self.bytes, &mut self.off, 7);
            self.bytes[self.off] = byte;
            self.off += 1;
            self.pairs += 1;
            self
        }

        /// The named key, declared type 9, with the element type the caller names --
        /// which is how an array of the wrong kind gets written.
        fn raw_array_header(&mut self, key: &[u8], elem_type: u32, count: u64) -> &mut Self {
            self.room(key.len() + 32);
            tw::put_key(&mut self.bytes, &mut self.off, key);
            tw::put_u32_at(&mut self.bytes, &mut self.off, 9);
            tw::put_u32_at(&mut self.bytes, &mut self.off, elem_type);
            tw::put_u64_at(&mut self.bytes, &mut self.off, count);
            self.pairs += 1;
            self
        }

        fn tokens(&mut self, values: &[&[u8]]) -> &mut Self {
            self.tokens_as(values.len() as u64, values)
        }

        fn tokens_as(&mut self, declared: u64, values: &[&[u8]]) -> &mut Self {
            self.room(values.len() * (MAX_PIECE_BYTES + 12) + 64);
            tw::put_vocab(
                &mut self.bytes,
                &mut self.off,
                b"tokenizer.ggml.tokens",
                declared,
                values,
            );
            self.pairs += 1;
            self
        }

        fn floats(&mut self, key: &[u8], values: &[f32]) -> &mut Self {
            self.room(key.len() + values.len() * 4 + 24);
            tw::put_f32_array(&mut self.bytes, &mut self.off, key, values);
            self.pairs += 1;
            self
        }

        fn ints(&mut self, key: &[u8], values: &[i32]) -> &mut Self {
            self.room(key.len() + values.len() * 4 + 24);
            tw::put_i32_array(&mut self.bytes, &mut self.off, key, values);
            self.pairs += 1;
            self
        }

        /// Closes the file: the header states the pairs actually written, so a test
        /// cannot disagree with itself about how many it sent.
        fn build(&mut self) -> Vec<u8> {
            tw::begin_file(&mut self.bytes, self.pairs);
            let mut bytes = core::mem::take(&mut self.bytes);
            bytes.truncate(self.off);
            bytes
        }
    }

    /// The installed file's shape, scaled down: 269 entries whose last 256 are the
    /// file's own byte tokens, with the scores and types arrays it declares.
    ///
    /// `pub(crate)` for the bridge below, which lets another module's test feed the
    /// inference engine ids that came out of a real vocabulary walk instead of
    /// numbers typed into the test.
    pub(crate) fn installed_shape() -> Vec<u8> {
        let tokens = vocabulary();
        let kinds = types();
        // Byte tokens score 0.0 here exactly as they do in the real file -- the
        // highest value in the array -- so the fixture exercises the trap the
        // matchable rule exists to avoid.
        let scores: Vec<f32> = (0..tokens.len())
            .map(|id| {
                if id < 5 {
                    -1000.0
                } else if id < 13 {
                    -1.0 - id as f32
                } else {
                    0.0
                }
            })
            .collect();

        let mut file = Fixture::new();
        file.string(b"general.architecture", b"gemma-embedding")
            .string(b"tokenizer.ggml.model", b"llama")
            .string(b"tokenizer.ggml.pre", b"default")
            .id(b"tokenizer.ggml.bos_token_id", 2)
            .id(b"tokenizer.ggml.eos_token_id", 1)
            .id(b"tokenizer.ggml.padding_token_id", 0)
            .id(b"tokenizer.ggml.unknown_token_id", 3)
            .flag(b"tokenizer.ggml.add_bos_token", true)
            .flag(b"tokenizer.ggml.add_eos_token", true)
            .flag(b"tokenizer.ggml.add_space_prefix", false)
            .tokens(&tokens)
            .floats(b"tokenizer.ggml.scores", &scores)
            .ints(b"tokenizer.ggml.token_type", &kinds);
        file.build()
    }

    fn table_from(bytes: &[u8]) -> Result<FileTokenizer, TokenizerError> {
        let mut stream = GgufStream::from_slice(bytes);
        FileTokenizer::build_from_stream(&mut stream)
    }
    fn installed() -> FileTokenizer {
        let bytes = installed_shape();
        table_from(&bytes).expect("the fixture builds a table")
    }

    /// Encodes and returns the ids, panicking with the refusal if the table said
    /// no -- every case below fits the buffer it is given.
    fn ids_of(text: &[u8]) -> Vec<u32> {
        let table = installed();
        let mut out = [0u32; 32];
        let taken = table
            .encode(text, &mut out)
            .unwrap_or_else(|error| panic!("{text:?} refused: {error:?}"));
        out[..taken].to_vec()
    }

    /// Encodes and decodes, so a claim about the table is a claim about bytes.
    fn round_trip(text: &[u8]) -> Vec<u8> {
        let table = installed();
        let mut out = [0u32; 32];
        let taken = table
            .encode(text, &mut out)
            .unwrap_or_else(|error| panic!("{text:?} refused: {error:?}"));
        let mut back = [0u8; 256];
        let written = table.decode_sequence(&out[..taken], &mut back);
        back[..written].to_vec()
    }

    #[test]
    fn test_the_table_is_built_from_the_file_s_own_arrays() {
        let table = installed();
        assert_eq!(table.declared_vocab(), 269);
        // The eight entries that are neither a control token nor a byte token.
        assert_eq!(table.matchable_pieces(), 8);
        assert!(table.declares_scores());
        assert!(table.declares_token_types());
        assert!(table.encodes_word_boundaries());
        assert_eq!(table.max_piece_len(), 10, "\u{2581}battery is the longest piece");

        // The byte map is the file's own: 256 entries taken from its `<0xNN>`
        // names, and the ids those names sit at -- not the offset 4 the invented
        // table hard-coded.
        assert!(table.has_byte_fallback());
        assert_eq!(table.byte_fallback_width(), 256);
        assert_eq!(table.byte_fallback_first_id(), Some(BYTE_BASE));
        assert_eq!(table.byte_id_of(0x00), Some(byte_id(0x00)));
        assert_eq!(table.byte_id_of(b' '), Some(byte_id(b' ')));
        assert_eq!(table.byte_id_of(0xFF), Some(byte_id(0xFF)));
        assert_eq!(table.byte_of_id(byte_id(0x01)), Some(0x01));
        assert_eq!(table.byte_of_id(byte_id(0xFF)), Some(0xFF));
        assert_eq!(table.byte_of_id(9), None, "a real piece is not a byte token");

        // The specials are the ids the file stated.
        assert_eq!(
            (table.bos_id(), table.eos_id(), table.pad_id(), table.unk_id()),
            (Some(2), Some(1), Some(0), Some(3))
        );
        assert_eq!(table.adds_bos_token(), Some(true));
        assert_eq!(table.adds_eos_token(), Some(true));
        assert_eq!(table.adds_space_prefix(), Some(false));
        assert_eq!(
            table.declares_byte_fallback_flag(),
            None,
            "this file carries no such key"
        );
    }

    #[test]
    fn test_encode_takes_the_longest_piece_the_file_declares() {
        // The leading bos id is the file's `add_bos_token` and its `bos_token_id`,
        // both: drop either in the fixture and it stops being emitted.
        assert_eq!(ids_of(b"battery"), vec![2, 9]);
        assert_eq!(ids_of(b"battery status"), vec![2, 9, 7]);
        assert_eq!(ids_of(b"status"), vec![2, 10]);
        assert_eq!(ids_of(b"ate"), vec![2, 11]);
        assert_eq!(ids_of(b"a"), vec![2, 12]);
        // `bate`: no piece starts with `b`, so that byte takes the file's own
        // `<0x62>` id and the rest of the word still matches as a piece.
        assert_eq!(ids_of(b"bate"), vec![2, byte_id(b'b'), 11]);
        assert_eq!(ids_of(b"q"), vec![2, byte_id(b'q')]);
        // A word boundary consumes the space that is in the input: id 6 is
        // `\u{2581}battery`, and the eight bytes it stands for are " battery".
        assert_eq!(ids_of(b" battery"), vec![2, 6]);
        assert_eq!(ids_of(b" is"), vec![2, 8]);
        // Without the space the marker piece is unreachable, and `is` is not a
        // piece this file declares: both bytes fall back.
        assert_eq!(ids_of(b"is"), vec![2, byte_id(b'i'), byte_id(b's')]);
    }

    #[test]
    fn test_every_encoding_decodes_back_to_the_bytes_that_were_sent() {
        for text in [
            "battery",
            "battery status",
            " battery",
            "  double  space",
            "a a a",
            "ate a status",
            "\u{0}",
            "\u{ff}\u{01}",
            "battery is ok",
            "naïve café — ünïcøde",
            " ",
            "",
        ] {
            assert_eq!(
                round_trip(text.as_bytes()),
                text.as_bytes(),
                "{text:?} did not survive its own id stream"
            );
        }
    }

    #[test]
    fn test_a_marker_in_the_input_is_read_as_the_marker_and_not_round_tripped() {
        // The limitation, asserted instead of hidden: the file says U+2581 is a
        // word boundary, so a request that already carries one is answered as if it
        // had carried a space. Only a normalisation stage the file does not declare
        // could tell the two cases apart -- see the module header.
        assert_eq!(round_trip("\u{2581}".as_bytes()), b" ");
        assert_eq!(ids_of("\u{2581}battery".as_bytes()), vec![2, 6]);
    }

    #[test]
    fn test_a_byte_token_decodes_to_the_byte_and_not_to_its_name() {
        let table = installed();
        let mut out = [0u8; 8];
        // The defect this pins shut: the id for 0x20 is named `<0x20>`, and copying
        // that name into the reply put six characters where a space belongs. A round
        // trip over the real file's vocabulary found exactly that.
        assert_eq!(table.decode_token(byte_id(b' '), &mut out), 1);
        assert_eq!(&out[..1], b" ");
        assert_eq!(table.decode_token(byte_id(0x00), &mut out), 1);
        assert_eq!(&out[..1], b"\0");
        // A special carries no text at all.
        assert_eq!(table.decode_token(2, &mut out), 0);
        assert_eq!(table.decode_token(1, &mut out), 0);
        // A control token the file named but never used as a special: never a
        // piece, and its name is all the text it holds.
        assert_eq!(table.decode_token(4, &mut out), 6);
        // An id the file never declared is refused by silence, not invented.
        assert_eq!(table.decode_token(9999, &mut out), 0);
    }

    #[test]
    fn test_a_word_marker_piece_decodes_to_a_space_and_not_to_the_marker() {
        let table = installed();
        let mut out = [0u8; 16];
        assert_eq!(table.decode_token(6, &mut out), 8);
        assert_eq!(&out[..8], b" battery");
        assert_eq!(table.decode_token(5, &mut out), 1);
        assert_eq!(&out[..1], b" ");
    }

    #[test]
    fn test_a_file_with_no_byte_tokens_says_it_has_none() {
        let mut file = Fixture::new();
        file.id(b"tokenizer.ggml.bos_token_id", 2)
            .id(b"tokenizer.ggml.unknown_token_id", 3)
            .flag(b"tokenizer.ggml.add_bos_token", true)
            .tokens(&[b"<pad>", b"<eos>", b"<bos>", b"<unk>", b"ab"])
            .ints(b"tokenizer.ggml.token_type", &[3, 3, 3, 3, 1]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("a file with pieces builds");
        assert!(!table.has_byte_fallback());
        assert_eq!(table.byte_fallback_width(), 0);
        assert_eq!(table.byte_fallback_first_id(), None);
        assert_eq!(table.byte_id_of(0x00), None);
        assert_eq!(table.byte_of_id(4), None);
        // A byte with no piece falls to the file's unk; the piece still matches.
        let mut out = [0u32; 8];
        let taken = table.encode(b"q", &mut out).expect("two positions fit eight");
        assert_eq!(&out[..taken], &[2, 3]);
        let mut out = [0u32; 8];
        let taken = table.encode(b"ab", &mut out).expect("two positions fit eight");
        assert_eq!(&out[..taken], &[2, 4]);
    }

    #[test]
    fn test_a_file_with_no_special_ids_says_what_it_cannot_encode() {
        // WHY this is not a refusal: a file with one piece and no specials and no
        // byte tokens can still encode the text it has a piece for, and the bytes it
        // has no way to name are dropped. `has_byte_fallback() == false` together
        // with `unk_id() == None` is what tells a reader that they were.
        let mut file = Fixture::new();
        file.tokens(&[b"ab"]).ints(b"tokenizer.ggml.token_type", &[1]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("one piece is a vocabulary");
        assert_eq!(table.unk_id(), None);
        assert_eq!(table.adds_bos_token(), None);
        let mut out = [0u32; 8];
        let taken = table.encode(b"abc", &mut out).expect("one position fits eight");
        assert_eq!(&out[..taken], &[0], "the `c` earned no id at all");
        let mut back = [0u8; 8];
        assert_eq!(table.decode_sequence(&out[..taken], &mut back), 2);
        assert_eq!(&back[..2], b"ab");
    }

    #[test]
    fn test_two_pieces_with_the_same_bytes_encode_with_the_better_scored_one() {
        // The file declares `z` twice, at ids 1 and 2, and scores the second one
        // lower. Both decode to `z`, so no round trip can see the difference --
        // which is exactly why the id is asserted here rather than the text.
        let mut file = Fixture::new();
        file.flag(b"tokenizer.ggml.add_bos_token", false)
            .id(b"tokenizer.ggml.unknown_token_id", 0)
            .tokens(&[b"<unk>", b"z", b"z"])
            .ints(b"tokenizer.ggml.token_type", &[3, 1, 1])
            .floats(b"tokenizer.ggml.scores", &[-100.0, -2.5, -9.0]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("duplicate pieces build");
        assert_eq!(table.matchable_pieces(), 2);
        let mut out = [0u32; 4];
        let taken = table.encode(b"z", &mut out).expect("one position fits four");
        assert_eq!(&out[..taken], &[1], "id 1 holds the higher score");
    }

    #[test]
    fn test_a_file_without_scores_ranks_identical_pieces_by_id() {
        let mut file = Fixture::new();
        file.id(b"tokenizer.ggml.unknown_token_id", 0)
            .flag(b"tokenizer.ggml.add_bos_token", false)
            .tokens(&[b"<unk>", b"z", b"z"])
            .ints(b"tokenizer.ggml.token_type", &[3, 1, 1]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("a scoreless vocabulary still builds");
        assert!(!table.declares_scores());
        let mut out = [0u32; 4];
        table.encode(b"z", &mut out).expect("one position fits four");
        assert_eq!(out[0], 1, "the lower id wins when nothing ranks them");
    }

    #[test]
    fn test_a_text_longer_than_the_buffer_is_refused_with_both_numbers() {
        let table = installed();
        let mut out = [0u32; 1];
        let error = table
            .encode(b"battery status", &mut out)
            .expect_err("three positions cannot go in one");
        assert_eq!(
            error,
            EncodeError::BeyondBuffer {
                needed: 3,
                bound: 1
            }
        );
        // And the refusal left the buffer exactly as it found it.
        assert_eq!(out[0], 0);
    }

    #[test]
    fn test_a_vocabulary_past_the_budget_is_refused_before_its_elements_are_read() {
        let declared = (TOKENIZER_HEAP_BUDGET / MIN_TABLE_BYTES_PER_ENTRY) + 1;
        let mut file = Fixture::new();
        file.tokens_as(declared as u64, &[b"<pad>"]);
        let bytes = file.build();
        let error = table_from(&bytes)
            .err()
            .expect("a declared vocabulary past the budget must not be walked");
        assert_eq!(
            error,
            TokenizerError::VocabTooLarge(
                declared * MIN_TABLE_BYTES_PER_ENTRY,
                TOKENIZER_HEAP_BUDGET
            )
        );
    }

    #[test]
    fn test_a_piece_longer_than_the_window_names_the_id_and_both_numbers() {
        let long = vec![b'x'; MAX_PIECE_BYTES + 1];
        let mut file = Fixture::new();
        file.tokens_as(2, &[b"a", &long]);
        let bytes = file.build();
        let error = table_from(&bytes)
            .err()
            .expect("a 129-byte piece is not a token");
        assert_eq!(
            error,
            TokenizerError::PieceTooLong(1, MAX_PIECE_BYTES + 1, MAX_PIECE_BYTES)
        );
    }

    #[test]
    fn test_a_declared_array_shorter_than_the_vocabulary_is_refused() {
        let tokens = vocabulary();
        let kinds = types();
        let mut file = Fixture::new();
        file.tokens(&tokens)
            .ints(b"tokenizer.ggml.token_type", &kinds)
            .floats(b"tokenizer.ggml.scores", &[-1.0, -2.0]);
        let bytes = file.build();
        let error = table_from(&bytes)
            .err()
            .expect("two scores cannot index 269 ids");
        assert_eq!(
            error,
            TokenizerError::ArrayLengthMismatch("tokenizer.ggml.scores", 2, 269)
        );
    }

    #[test]
    fn test_a_file_that_declares_no_vocabulary_is_refused() {
        let mut file = Fixture::new();
        file.string(b"general.architecture", b"gemma-embedding")
            .id(b"tokenizer.ggml.bos_token_id", 2);
        let bytes = file.build();
        assert_eq!(table_from(&bytes).err(), Some(TokenizerError::NoVocabulary));
    }

    #[test]
    fn test_a_file_whose_entries_may_never_be_emitted_is_refused() {
        // Every entry CONTROL: a vocabulary with no matchable piece cannot encode
        // one character of text, and answering from it would be answering from
        // nothing.
        let mut file = Fixture::new();
        file.tokens(&[b"<pad>", b"<eos>"])
            .ints(b"tokenizer.ggml.token_type", &[3, 3]);
        let bytes = file.build();
        assert_eq!(table_from(&bytes).err(), Some(TokenizerError::NoVocabulary));
    }

    #[test]
    fn test_a_file_that_stops_inside_the_vocabulary_names_the_key() {
        let mut file = Fixture::new();
        file.tokens_as(8, &[b"a", b"b"]);
        let bytes = file.build();
        assert_eq!(
            table_from(&bytes).err(),
            Some(TokenizerError::Truncated("tokenizer.ggml.tokens"))
        );
    }

    #[test]
    fn test_a_flag_that_is_not_a_boolean_is_refused_as_metadata() {
        let mut file = Fixture::new();
        file.tokens(&[b"a"])
            .raw_flag(b"tokenizer.ggml.add_bos_token", 17);
        let bytes = file.build();
        assert_eq!(
            table_from(&bytes).err(),
            Some(TokenizerError::Reader(GgufError::InvalidMetadata))
        );
    }

    #[test]
    fn test_a_string_where_a_flag_belongs_is_refused_as_metadata() {
        let mut file = Fixture::new();
        file.tokens(&[b"a"])
            .string(b"tokenizer.ggml.add_bos_token", b"yes");
        let bytes = file.build();
        assert_eq!(
            table_from(&bytes).err(),
            Some(TokenizerError::Reader(GgufError::InvalidMetadata))
        );
    }

    #[test]
    fn test_a_scores_array_of_the_wrong_element_type_is_refused() {
        let mut file = Fixture::new();
        file.tokens(&[b"a"])
            .raw_array_header(b"tokenizer.ggml.scores", 8, 1);
        let bytes = file.build();
        assert_eq!(
            table_from(&bytes).err(),
            Some(TokenizerError::BadArrayShape(
                "tokenizer.ggml.scores element type",
                8
            ))
        );
    }

    #[test]
    fn test_another_family_s_scores_are_not_read_as_this_file_s() {
        // `bert.scores` is an f32 array whose length would fail the vocabulary gate.
        // A walk that matched keys by suffix would refuse this file for a key it
        // never sent, so the array is stepped over and the build reports that no
        // scores were declared.
        let mut file = Fixture::new();
        file.tokens(&[b"a", b"b"])
            .floats(b"bert.scores", &[-1.0, -2.0]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("a foreign array is stepped over");
        assert!(!table.declares_scores());
        assert_eq!(table.declared_vocab(), 2);
    }

    #[test]
    fn test_a_key_longer_than_the_reader_names_is_stepped_over() {
        let long_key = vec![b't'; gguf::MAX_METADATA_KEY_LEN + 8];
        let mut file = Fixture::new();
        file.string(&long_key, b"ignored").tokens(&[b"a"]);
        let bytes = file.build();
        let table = table_from(&bytes).expect("an odd key is not a fault");
        assert_eq!(table.declared_vocab(), 1);
    }

    #[test]
    fn test_the_table_reports_what_it_holds_against_the_budget() {
        let table = installed();
        assert!(table.heap_bytes() <= TOKENIZER_HEAP_BUDGET);
        // The fixture's whole table is a few kilobytes; the number exists so the
        // real-file test below can print the same one for a real vocabulary.
        assert!(table.heap_bytes() < 64 * 1024, "held {}", table.heap_bytes());
    }

    /// The installed file's own vocabulary, built from its own bytes, round trips
    /// its own pieces.
    ///
    /// WHAT THIS DOES NOT DO, AND CANNOT ON THIS HOST: it compares no id against any
    /// reference runtime. No sentencepiece, llama.cpp or torch tokenizer is
    /// installed here, so a claim that these are the ids Gemma's own tokenizer would
    /// give is not checkable, and an assertion that said it would be the same
    /// fabrication the invented table was. What is asserted is that every id came from
    /// this file and may be emitted by it, that the segmentation decodes back to the
    /// bytes that were sent, and that the table is what the budget was sized against.
    /// The parity gap is stated in the module header.
    #[test]
    fn test_the_real_file_encodes_its_own_vocabulary() {
        let Some(path) = host::env::var("INTEROS_REAL_GGUF_PATH").ok() else {
            host::println!(
                "NOT VERIFIED against a real model file: INTEROS_REAL_GGUF_PATH is unset, so \
                 the installed vocabulary was never read and only hand-written fixtures were \
                 encoded. The ids of the real 262,144-token table therefore go unasserted."
            );
            return;
        };
        use host::io::Read;

        // This file's metadata block -- 262,144 pieces and their scores and types --
        // runs to a little under seven megabytes, so twelve holds all of it and the
        // walk gets to read the real thing rather than a fixture of it.
        const WINDOW: usize = 12 * 1024 * 1024;
        let mut buffer: host::vec::Vec<u8> = host::vec![0u8; WINDOW];
        let mut file = host::fs::File::open(&path)
            .unwrap_or_else(|error| panic!("the real file at {path} could not be opened: {error}"));
        let mut filled = 0usize;
        while filled < WINDOW {
            match file.read(&mut buffer[filled..]) {
                Ok(0) => break,
                Ok(got) => filled += got,
                Err(error) => panic!("the real file at {path} could not be read: {error}"),
            }
        }
        buffer.truncate(filled);

        let started = host::time::Instant::now();
        let table = table_from(&buffer)
            .unwrap_or_else(|error| panic!("the real file at {path} gave no table: {error:?}"));
        let build_time = started.elapsed();

        assert_eq!(table.declared_vocab(), 262_144);
        assert!(table.declares_scores());
        assert!(table.declares_token_types());
        assert_eq!(table.matchable_pieces(), 255_637);
        assert_eq!(table.bos_id(), Some(2));
        assert_eq!(table.eos_id(), Some(1));
        assert_eq!(table.pad_id(), Some(0));
        assert_eq!(table.unk_id(), Some(3));
        assert_eq!(table.adds_bos_token(), Some(true));
        assert_eq!(
            table.adds_eos_token(),
            Some(true),
            "read, reported, not applied"
        );
        assert_eq!(
            table.adds_space_prefix(),
            Some(false),
            "read, reported, not applied"
        );
        assert!(table.encodes_word_boundaries());
        assert_eq!(table.max_piece_len(), 48);

        // The byte block the file actually has: 256 tokens at 238..=493. The
        // invented table said the byte offset was 4.
        assert!(table.has_byte_fallback());
        assert_eq!(table.byte_fallback_width(), 256);
        assert_eq!(table.byte_fallback_first_id(), Some(238));
        assert_eq!(table.byte_id_of(0x00), Some(238));
        assert_eq!(table.byte_id_of(b' '), Some(270));
        assert_eq!(table.byte_id_of(0xFF), Some(493));
        assert_eq!(table.byte_of_id(238), Some(0x00));
        assert_eq!(table.byte_of_id(493), Some(0xFF));

        // The two numbers the budget was chosen against.
        assert!(
            table.heap_bytes() <= TOKENIZER_HEAP_BUDGET,
            "the real file's table held {} of the {TOKENIZER_HEAP_BUDGET} bytes allowed",
            table.heap_bytes()
        );

        for text in [
            "battery status",
            "The system battery status is ok",
            "the model is ready",
            "query: battery consumption profile",
            "InterOS messages offline 42%",
            "  double  space",
            "naïve café — ünïcøde",
        ] {
            let mut out = [0u32; 64];
            let taken = table
                .encode(text.as_bytes(), &mut out)
                .unwrap_or_else(|error| panic!("{text:?} refused: {error:?}"));
            assert!(taken > 0, "{text:?} encoded to nothing");
            for &id in &out[..taken] {
                // Three kinds of id may legitimately come out of `encode`: a piece
                // the file lets be matched, one of its `<0xNN>` byte tokens (which
                // the table excludes from matching on purpose and emits only as
                // fallback), and the bos the file's own flag asked for. Anything
                // else is an id this file never named.
                assert!(
                    table.may_emit(id) || table.byte_of_id(id).is_some() || table.is_special(id),
                    "{text:?} emitted id {id}, which this file does not let be emitted"
                );
            }
            let mut back = [0u8; 512];
            let written = table.decode_sequence(&out[..taken], &mut back);
            assert_eq!(
                &back[..written],
                text.as_bytes(),
                "{text:?} -> ids -> {:?}",
                host::str::from_utf8(&back[..written])
            );
            host::println!(
                "REAL FILE {text:?}: {taken} ids, opening ids {:?}",
                &out[..taken.min(3)]
            );
        }

        host::println!(
            "REAL FILE {path}: vocab {}, {} matchable pieces, {} byte tokens from id {}, \
             table holds {} of the {TOKENIZER_HEAP_BUDGET} bytes allowed; one build from a \
             {WINDOW}-byte prefix took {build_time:?}. NOT compared to any reference \
             tokenizer, because none is installed on this host.",
            table.declared_vocab(),
            table.matchable_pieces(),
            table.byte_fallback_width(),
            table.byte_fallback_first_id().unwrap_or(0),
            table.heap_bytes(),
        );
    }
}
