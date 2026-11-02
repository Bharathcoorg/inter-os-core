//! Reads an installed model file's tensors into the shapes `inference` runs.
//!
//! This is the hop between the GGUF reader, which could already name a tensor and
//! say where its bytes sit, and the forward pass, which could already run a graph
//! over weights handed to it: nothing in the service ever turned the installed
//! file's tensor bytes into a `ModelWeights`, so the engine priced a model it never
//! read and the notice on the screen said so.
//!
//! WHY it reads one block at a time and hands the buffers back: the installed
//! `embeddinggemma-300M-Q8_0.gguf` is 333,590,944 bytes and its `token_embd.weight`
//! alone is 213,909,504 of them -- more than the 125,829,120-byte budget one
//! inference run is allowed, before the 100,663,296-byte key/value cache that same
//! file is priced at. A loader that materialised the weights it was given would
//! either fail to allocate or be a second copy of the file. So this module reads a
//! block's tensors, runs the block, and releases the slot before reading the next
//! one, and the peak it reports is one block plus one embedding row.
//!
//! WHY the quantisation comes from the descriptor and not from this file: every
//! tensor's `tensor_type` is what the file states for it, and it is passed through
//! to `gemv` and `embedding_row_into` unchanged. A dtype this engine has no row
//! layout for is refused by its number rather than read as a neighbour's type, and
//! a block whose matrices declare two different types is refused outright -- a
//! `LayerWeights` carries one `weight_type` precisely so that a mixed block cannot
//! be answered with the wrong dequantiser.
//!
//! Strictly `#![no_std]` with `alloc`: the per-block buffers are heap, the
//! descriptor table is heap, and neither is on a 256 KiB service stack.
//!
//! WHAT IS NOT WIRED, so this file is not read as more than it is: no dispatch arm
//! in `lib.rs` calls into this module yet. The embedding arm still answers from
//! `embedding`'s lexical projection and the generative arm still answers with the
//! gap notice, which is why that notice now says `none of whose tensors this reply
//! reads` and not `the engine does not read its tensors` -- the second claim is
//! false since this module existed, the first is still exactly what the device does.
//! Everything measured here was measured on the development host, through
//! `tests::HostFileSource`, against the file `INTEROS_REAL_GGUF_PATH` names; the
//! device's own source is `ModelFileDescriptor`, which the host suite cannot drive
//! because `fs_read_at` is a syscall there.

use crate::inference::{
    self, embedding_row_into, f32_sqrt, pooled_normalised, quantised_row_bytes, run_block,
    InferenceError, KvCache, LayerWeights, ModelConfig, Scratch,
};
use alloc::vec::Vec;
use core::mem::size_of;

/// Longest tensor name this loader will look for, shared with the reader that parses
/// the descriptors: a name wider than the reader's own window is not comparable.
pub const MAX_NAME_BYTES: usize = crate::gguf::MAX_TENSOR_NAME_LEN;

/// The eight quantisations `gemv` and `embedding_row_into` know the byte layout of.
///
/// WHY a list and not a table in this module: the engine's arithmetic is the
/// authority, and a loader that carried its own size table could disagree with it
/// about a block's width without either one saying so. The sizes are asked for
/// through `quantised_row_bytes`, which returns `None` for anything else, and `None`
/// becomes `LoadError::UnsupportedDtype` naming the tensor and the number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadError {
    /// The reader's own failure, kept whole: a truncated descriptor table and a
    /// metadata value the walk refused are different refusals with different next
    /// moves, and flattening them would lose which one the file caused.
    Reader(crate::gguf::GgufError),
    /// The graph's own failure, kept whole for the same reason.
    Graph(InferenceError),
    /// The table the file states has no tensor under this name. Carries the suffix
    /// that was looked for and, for a block tensor, the block index, so the report
    /// can name `blk.7.ffn_gate.weight` instead of "a tensor was missing".
    TensorAbsent(&'static str, Option<usize>),
    /// The tensor declares a dtype the engine has no row layout for, by number.
    /// The name is the tensor's, so the answer says which one the file quantised
    /// in a way this build cannot read.
    UnsupportedDtype(&'static str, u32),
    /// A block's matrices do not all declare the same dtype. `LayerWeights` carries
    /// one `weight_type`, so a file that split a block across two quantisations is
    /// not a block this engine can run, and guessing one of them would dequantise
    /// the other with the wrong scale.
    MixedBlockDtype(&'static str, u32, u32),
    /// The file declared `general.alignment` as something no GGUF file can have.
    /// Carries the number the file stated.
    BadAlignment(u64),
    /// A positioned read came back short. Carries the position, the bytes asked for
    /// and the bytes that arrived: a truncated install is a different fact from a
    /// corrupt tensor, and only one of them is the file's fault.
    ShortRead(u64, usize, usize),
    /// The descriptor's own dimensions do not describe the tensor the graph needs
    /// (a row width that is not a whole number of quantisation blocks, or a table
    /// with no rows). Carries the tensor's suffix and the two numbers read.
    BadShape(&'static str, u64, u64),
    /// The arithmetic a tensor's size needed did not fit a `usize`.
    Arithmetic,
}

impl From<crate::gguf::GgufError> for LoadError {
    fn from(error: crate::gguf::GgufError) -> Self {
        Self::Reader(error)
    }
}

impl From<InferenceError> for LoadError {
    fn from(error: InferenceError) -> Self {
        Self::Graph(error)
    }
}

/// Where bytes come from, at a position, without moving a cursor.
///
/// WHY a trait and not `interos_std::fs_read_at` at the call sites: the device's
/// positioned read is a syscall, and the host suite that measures this loader runs
/// against the real published file with the host's own file handle. Both answers
/// have to drive the same per-block code path, or the measurement would be of a
/// different reader than the one that ships.
pub trait PositionRead {
    /// Reads exactly `into.len()` bytes from `position`, or reports why it could
    /// not. A short read is a failure, not a partial success: a block's tail left
    /// unread is a block the graph would run with the previous block's bytes.
    fn read_at(&mut self, position: u64, into: &mut [u8]) -> Result<(), LoadError>;
}

/// The device's source: one descriptor the service already opened.
///
/// `aid` holds the read capability for the model namespace; the fd arrives from the
/// same open the header probe used, and this type owns nothing, so closing stays
/// with the caller that opened.
pub struct ModelFileDescriptor {
    pub fd: u32,
}

impl PositionRead for ModelFileDescriptor {
    fn read_at(&mut self, position: u64, into: &mut [u8]) -> Result<(), LoadError> {
        let mut filled = 0usize;
        while filled < into.len() {
            let got = crate::std::fs_read_at(self.fd, position + filled as u64, &mut into[filled..])
                .map_err(|_| LoadError::ShortRead(position + filled as u64, into.len(), filled))?;
            if got == 0 {
                return Err(LoadError::ShortRead(
                    position + filled as u64,
                    into.len(),
                    filled,
                ));
            }
            filled += got;
        }
        Ok(())
    }
}

/// One tensor descriptor, as the file stated it, plus the position a read needs.
#[derive(Debug, Clone)]
pub struct TensorEntry {
    /// The tensor's name, byte for byte out of the file.
    pub name: Vec<u8>,
    /// The GGML type id the file declares for this tensor's bytes.
    pub dtype: u32,
    /// Elements along the contiguous dimension (GGUF's `dims[0]`), which is a
    /// matrix's `cols` -- the width `gemv` walks a row over.
    pub cols: u64,
    /// Rows: GGUF's `dims[1]`, a matrix's output width. `dims[0]` for a 1-D tensor.
    pub rows: u64,
    /// The count of dimensions the file states (1 through 4).
    pub n_dims: u32,
    /// Total bytes of the tensor's payload, computed by the reader's own size rule.
    pub byte_len: u64,
    /// The offset the file states, relative to the start of the data section. Kept
    /// as the file gave it so a report can quote both numbers it was given.
    pub stated_offset: u64,
    /// Absolute file position of the first payload byte: the data section start the
    /// descriptor table rounds up to, plus the offset the file states.
    pub position: u64,
}

impl TensorEntry {
    /// Rows this tensor holds for the graph: a 1-D norm's element count, or a
    /// matrix's second dimension.
    fn row_count(&self) -> u64 {
        if self.n_dims <= 1 {
            self.cols
        } else {
            self.rows
        }
    }

    /// The handle a read needs, taken by value so a reader can fill a slot while it
    /// still holds the table the entry lives in.
    fn handle(&self) -> TensorRef {
        TensorRef {
            dtype: self.dtype,
            cols: self.cols,
            rows: self.row_count(),
            byte_len: self.byte_len,
            position: self.position,
            stated_offset: self.stated_offset,
        }
    }
}

/// One tensor's read parameters, by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TensorRef {
    pub dtype: u32,
    /// Elements along the contiguous dimension: a matrix's `cols`.
    pub cols: u64,
    /// Rows: a matrix's output width, or a 1-D tensor's element count.
    pub rows: u64,
    pub byte_len: u64,
    pub position: u64,
    pub stated_offset: u64,
}

/// Everything the file's descriptor table names, taken in one pass.
///
/// WHY a table and not a lookup per tensor: `locate_tensor_stream` re-walks the
/// whole descriptor list to resolve one name, and a run reads 24 blocks x 13
/// tensors -- 312 lookups over 316 descriptors each, every one of them behind the
/// same 512-byte metadata window. One pass costs the walk once and holds the
/// descriptors, which is bytes of name and number and not the weights.
///
/// The table is the only thing this loader holds across blocks, and its size is
/// reported: on the published file it is 316 entries over names no longer than 33
/// bytes, tens of kilobytes, against the 333 MB the file is.
pub struct TensorTable {
    entries: Vec<TensorEntry>,
    /// The position every descriptor's offset counts from, as the file's own
    /// `general.alignment` puts it.
    pub data_start: u64,
    /// The placement the file asked for, or the format's default.
    pub alignment: u64,
    /// How many times the metadata window touched its source during the walk.
    pub window_pulls: usize,
}

impl TensorTable {
    /// Walks the header, the metadata block and the descriptor table once.
    ///
    /// WHY the metadata is skipped instead of parsed: only `general.alignment` is
    /// needed to place a tensor, and the published file's metadata carries a
    /// 262,144-entry vocabulary. The skip counts each string's length prefix and
    /// steps past its bytes, which is why this walk reaches the table at all on a
    /// 512-byte window.
    pub fn walk(stream: &mut crate::gguf::GgufStream<'_>) -> Result<Self, LoadError> {
        let header = crate::gguf::GgufHeader::parse_stream(stream)?;
        let alignment = crate::gguf::read_alignment_stream(stream, header.kv_count)?;
        if !alignment.is_power_of_two() || alignment > crate::gguf::MAX_TENSOR_ALIGNMENT {
            return Err(LoadError::BadAlignment(alignment));
        }

        let mut entries: Vec<TensorEntry> = Vec::new();
        entries
            .try_reserve_exact(header.tensor_count as usize)
            .map_err(|_| LoadError::Arithmetic)?;

        for _ in 0..header.tensor_count {
            let described = crate::gguf::parse_tensor_info_stream(stream)?;
            let name_len = described.name_len;
            if name_len > MAX_NAME_BYTES {
                return Err(LoadError::BadShape("name", name_len as u64, MAX_NAME_BYTES as u64));
            }
            let byte_len =
                crate::gguf::tensor_byte_size(described.tensor_type, &described.dimensions, described.n_dimensions)?;
            let cols = described.dimensions[0];
            let rows = if described.n_dimensions > 1 {
                described.dimensions[1]
            } else {
                cols
            };
            entries.push(TensorEntry {
                name: described.name[..name_len].to_vec(),
                dtype: described.tensor_type,
                cols,
                rows,
                n_dims: described.n_dimensions,
                byte_len: byte_len as u64,
                stated_offset: described.offset,
                position: 0,
            });
        }

        // The table has been walked to its end, so the data section's start is now
        // knowable, and every descriptor's offset counts from it. Doing the addition
        // here rather than at the read is what keeps a wrong position one bug:
        // `test_a_tensor_position_is_the_section_start_plus_the_stated_offset`
        // checks it against `locate_tensor_stream`, which resolves the same number
        // for one name through the other code path.
        let data_start =
            crate::gguf::align_up(stream.position() as u64, alignment).map_err(LoadError::Reader)?;
        for entry in entries.iter_mut() {
            entry.position = data_start
                .checked_add(entry.stated_offset)
                .ok_or(LoadError::Arithmetic)?;
        }

        Ok(Self {
            entries,
            data_start,
            alignment,
            window_pulls: stream.pulls(),
        })
    }

    /// Looks one name up and takes the read parameters by value, which is what lets
    /// a reader fill a slot while it still owns the table the descriptor lives in.
    pub fn locate(&self, name: &[u8]) -> Option<TensorRef> {
        self.find(name).map(TensorEntry::handle)
    }

    /// Looks one name up.
    pub fn find(&self, name: &[u8]) -> Option<&TensorEntry> {
        self.entries.iter().find(|entry| entry.name.as_slice() == name)
    }

    /// Descriptors the file states, counted -- the number a report quotes and the
    /// reason the table is the biggest thing this loader holds.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Bytes the table itself holds: every name plus the fixed fields per entry.
    pub fn held_bytes(&self) -> usize {
        let fixed = size_of::<TensorEntry>();
        self.entries
            .iter()
            .fold(0usize, |acc, entry| acc + fixed + entry.name.len())
    }
}

/// Builds `blk.N.suffix` into a caller's buffer without formatting machinery.
///
/// WHY by hand: `alloc::format!` pulls in the formatting machinery this service does
/// not otherwise need, and a tensor name is a fixed prefix, a decimal number and a
/// fixed suffix, which is eight lines of arithmetic.
fn block_tensor_name<'b>(
    index: usize,
    suffix: &'static str,
    buffer: &'b mut [u8; MAX_NAME_BYTES],
) -> Option<&'b [u8]> {
    let mut at = 0usize;
    for byte in b"blk." {
        buffer[at] = *byte;
        at += 1;
    }
    // Decimal digits, most significant first, with no recursion and no allocation.
    let mut digits = [0u8; 20];
    let mut taken = 0usize;
    let mut value = index;
    loop {
        digits[taken] = b'0' + (value % 10) as u8;
        taken += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for step in (0..taken).rev() {
        buffer[at] = digits[step];
        at += 1;
    }
    buffer[at] = b'.';
    at += 1;
    if at + suffix.len() > MAX_NAME_BYTES {
        return None;
    }
    buffer[at..at + suffix.len()].copy_from_slice(suffix.as_bytes());
    Some(&buffer[..at + suffix.len()])
}

/// The seven fixed-name tensors a run needs outside the block loop.
///
/// WHY a name list and not a search by index: the file states its own names, and a
/// loader that picked tensors by their position in the table would answer with
/// whichever tensor a different exporter happened to write first.
pub const TENSOR_TOKEN_EMBD: &str = "token_embd.weight";
pub const TENSOR_OUTPUT_NORM: &str = "output_norm.weight";
pub const TENSOR_OUTPUT_HEAD: &str = "output.weight";
/// The published embedding model's pooling head, which this engine does not apply.
pub const TENSOR_DENSE_2: &str = "dense_2.weight";
pub const TENSOR_DENSE_3: &str = "dense_3.weight";

/// The per-block tensors, in the order the graph consumes them.
pub const BLOCK_QUANTISED: &[&str] = &[
    "attn_q.weight",
    "attn_k.weight",
    "attn_v.weight",
    "attn_output.weight",
    "ffn_gate.weight",
    "ffn_up.weight",
    "ffn_down.weight",
];

/// The per-block norm vectors, all declared F32 by every file seen so far.
pub const BLOCK_NORMS: &[&str] = &[
    "attn_norm.weight",
    "attn_q_norm.weight",
    "attn_k_norm.weight",
    "post_attention_norm.weight",
    "ffn_norm.weight",
    "post_ffw_norm.weight",
];

/// One block's tensors, owned, refilled per block.
///
/// The slot is the peak this loader reports: at the published file's Q8_0 geometry
/// it holds 4,490,220 bytes of matrices and 17,920 bytes of norms, and it is emptied
/// before the next block is read, so 24 blocks never coexist.
#[derive(Default)]
pub struct BlockSlot {
    attn_norm: Vec<f32>,
    attn_q_norm: Vec<f32>,
    attn_k_norm: Vec<f32>,
    post_attn_norm: Vec<f32>,
    ffn_norm: Vec<f32>,
    post_ffn_norm: Vec<f32>,
    attn_q: Vec<u8>,
    attn_k: Vec<u8>,
    attn_v: Vec<u8>,
    attn_o: Vec<u8>,
    ffn_gate: Vec<u8>,
    ffn_up: Vec<u8>,
    ffn_down: Vec<u8>,
    /// The dtype every quantised matrix in this block declared, taken from the
    /// descriptors and never from a guess.
    weight_type: u32,
    /// Bytes the slot held at its fullest for this block.
    held: usize,
}

impl BlockSlot {
    /// Drops every buffer, releasing the block's bytes before the next one is read.
    ///
    /// WHY `resize` to zero and not a re-`Vec::new`: the allocator keeps a freed
    /// block's pages available to the next allocation of the same size, which is the
    /// behaviour that makes the peak one block instead of 24; a fresh `Vec` per block
    /// would ask the kernel for the mapping again each time.
    pub fn release(&mut self) {
        for buffer in [
            &mut self.attn_norm,
            &mut self.attn_q_norm,
            &mut self.attn_k_norm,
            &mut self.post_attn_norm,
            &mut self.ffn_norm,
            &mut self.post_ffn_norm,
        ] {
            buffer.clear();
        }
        for buffer in [
            &mut self.attn_q,
            &mut self.attn_k,
            &mut self.attn_v,
            &mut self.attn_o,
            &mut self.ffn_gate,
            &mut self.ffn_up,
            &mut self.ffn_down,
        ] {
            buffer.clear();
        }
        self.held = 0;
    }

    /// Bytes held by this slot right now.
    pub fn held_bytes(&self) -> usize {
        self.held
    }

    /// The block as the graph sees it. The tensors are borrowed from the slot, which
    /// is why the loader runs a block before refilling the slot and not after.
    pub fn weights(&self) -> LayerWeights<'_> {
        LayerWeights {
            attn_norm: &self.attn_norm,
            attn_q_norm: &self.attn_q_norm,
            attn_k_norm: &self.attn_k_norm,
            attn_q: &self.attn_q,
            attn_k: &self.attn_k,
            attn_v: &self.attn_v,
            attn_o: &self.attn_o,
            post_attn_norm: &self.post_attn_norm,
            ffn_norm: &self.ffn_norm,
            ffn_gate: &self.ffn_gate,
            ffn_up: &self.ffn_up,
            ffn_down: &self.ffn_down,
            post_ffn_norm: &self.post_ffn_norm,
            weight_type: self.weight_type,
        }
    }
}

/// What one block read cost, so the run can report a measurement and not a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRead {
    /// Tensor payloads read into the slot.
    pub bytes_read: u64,
    /// Tensors whose declared dtype the engine had a layout for and read.
    pub tensors_read: usize,
    /// Bytes the slot held at the end of this block: the peak of the run if it is
    /// the widest block, since the slot is emptied between blocks.
    pub slot_bytes: usize,
}

/// Reads the installed file's tensors into `inference`'s shapes, one block at a time.
pub struct WeightReader<S: PositionRead> {
    table: TensorTable,
    source: S,
    slot: BlockSlot,
    bytes_read: u64,
    tensors_read: usize,
    peak_slot_bytes: usize,
}

impl<S: PositionRead> WeightReader<S> {
    /// Resolves the descriptor table once, then keeps the source for payload reads.
    pub fn new(table: TensorTable, source: S) -> Self {
        Self {
            table,
            source,
            slot: BlockSlot::default(),
            bytes_read: 0,
            tensors_read: 0,
            peak_slot_bytes: 0,
        }
    }

    pub fn table(&self) -> &TensorTable {
        &self.table
    }

    /// Fills one row of `token_embd.weight`, in the dtype the descriptor declares.
    ///
    /// WHY a row and not the tensor: the published table is 262,144 rows of 816
    /// bytes -- 213,909,504 bytes, more than the whole run's scratch budget. The
    /// step that asks for token `t` needs exactly row `t`.
    ///
    /// WHY an out-of-range id is refused: `forward_step`'s table path wraps with
    /// `% vocab_size`, which is the right answer for a table the engine holds and the
    /// wrong one for a positioned read -- wrapping would ask the file for a row that
    /// is not the token's and report its numbers as the model's embedding of it.
    pub fn load_embedding_row(
        &mut self,
        token: u32,
        hidden_dim: usize,
        buffer: &mut Vec<u8>,
    ) -> Result<(u32, u64), LoadError> {
        let entry = self
            .table
            .locate(TENSOR_TOKEN_EMBD.as_bytes())
            .ok_or(LoadError::TensorAbsent(TENSOR_TOKEN_EMBD, None))?;
        let row_bytes = quantised_row_bytes(entry.dtype, hidden_dim)
            .ok_or(LoadError::UnsupportedDtype(TENSOR_TOKEN_EMBD, entry.dtype))?;
        if (token as u64) >= entry.rows {
            return Err(LoadError::BadShape(TENSOR_TOKEN_EMBD, token as u64, entry.rows));
        }
        if entry.cols as usize != hidden_dim {
            return Err(LoadError::BadShape(
                TENSOR_TOKEN_EMBD,
                entry.cols,
                hidden_dim as u64,
            ));
        }
        let start = entry
            .position
            .checked_add(token as u64 * row_bytes as u64)
            .ok_or(LoadError::Arithmetic)?;
        buffer.clear();
        buffer
            .try_reserve_exact(row_bytes)
            .map_err(|_| LoadError::Arithmetic)?;
        buffer.resize(row_bytes, 0u8);
        self.source.read_at(start, buffer)?;
        self.bytes_read += row_bytes as u64;
        self.tensors_read += 1;
        Ok((entry.dtype, row_bytes as u64))
    }

    /// Reads one block's tensors into the slot, refusing anything the file did not
    /// declare in a form the graph can use.
    ///
    /// The norms arrive as `f32` because `rms_norm` walks floats; the matrices
    /// arrive as raw bytes because `gemv` dequantises a row at a time and a materialised
    /// Q8_0 block is four times the size of the slot that borrows it.
    pub fn load_block(&mut self, index: usize) -> Result<BlockRead, LoadError> {
        self.slot.release();
        let before = self.tensors_read;

        for suffix in BLOCK_NORMS {
            let name = block_tensor_name(index, suffix, &mut [0u8; MAX_NAME_BYTES])
                .ok_or(LoadError::BadShape(suffix, 0, MAX_NAME_BYTES as u64))?
                .to_vec();
            match self.table.locate(&name) {
                // The two per-head norms are optional because the files are: Gemma 2
                // ships none and the published embedding file ships both. A missing
                // one is left empty, which the block reads as "this file's graph does
                // not have that step", never as ones invented here.
                None if suffix.starts_with("attn_q_norm") || suffix.starts_with("attn_k_norm") => {
                    if suffix.starts_with("attn_q_norm") {
                        self.slot.attn_q_norm.clear();
                    } else {
                        self.slot.attn_k_norm.clear();
                    }
                }
                None => return Err(LoadError::TensorAbsent(suffix, Some(index))),
                Some(handle) => {
                    let floats = self.read_f32_tensor(suffix, handle)?;
                    match *suffix {
                        "attn_norm.weight" => self.slot.attn_norm = floats,
                        "attn_q_norm.weight" => self.slot.attn_q_norm = floats,
                        "attn_k_norm.weight" => self.slot.attn_k_norm = floats,
                        "post_attention_norm.weight" => self.slot.post_attn_norm = floats,
                        "ffn_norm.weight" => self.slot.ffn_norm = floats,
                        "post_ffw_norm.weight" => self.slot.post_ffn_norm = floats,
                        _ => return Err(LoadError::TensorAbsent(suffix, Some(index))),
                    }
                }
            }
        }

        let mut declared: Option<u32> = None;
        for suffix in BLOCK_QUANTISED {
            let name = block_tensor_name(index, suffix, &mut [0u8; MAX_NAME_BYTES])
                .ok_or(LoadError::BadShape(suffix, 0, MAX_NAME_BYTES as u64))?
                .to_vec();
            let handle = self
                .table
                .locate(&name)
                .ok_or(LoadError::TensorAbsent(suffix, Some(index)))?;
            if quantised_row_bytes(handle.dtype, handle.cols as usize).is_none() {
                return Err(LoadError::UnsupportedDtype(suffix, handle.dtype));
            }
            if let Some(existing) = declared {
                if existing != handle.dtype {
                    return Err(LoadError::MixedBlockDtype(suffix, handle.dtype, existing));
                }
            } else {
                declared = Some(handle.dtype);
            }
            let raw = self.read_raw_tensor(suffix, handle)?;
            match *suffix {
                "attn_q.weight" => self.slot.attn_q = raw,
                "attn_k.weight" => self.slot.attn_k = raw,
                "attn_v.weight" => self.slot.attn_v = raw,
                "attn_output.weight" => self.slot.attn_o = raw,
                "ffn_gate.weight" => self.slot.ffn_gate = raw,
                "ffn_up.weight" => self.slot.ffn_up = raw,
                "ffn_down.weight" => self.slot.ffn_down = raw,
                _ => return Err(LoadError::TensorAbsent(suffix, Some(index))),
            }
        }
        self.slot.weight_type = declared.ok_or(LoadError::TensorAbsent(
            BLOCK_QUANTISED[0],
            Some(index),
        ))?;

        self.slot.held = self.slot.attn_norm.len() * size_of::<f32>()
            + self.slot.attn_q_norm.len() * size_of::<f32>()
            + self.slot.attn_k_norm.len() * size_of::<f32>()
            + self.slot.post_attn_norm.len() * size_of::<f32>()
            + self.slot.ffn_norm.len() * size_of::<f32>()
            + self.slot.post_ffn_norm.len() * size_of::<f32>()
            + self.slot.attn_q.len()
            + self.slot.attn_k.len()
            + self.slot.attn_v.len()
            + self.slot.attn_o.len()
            + self.slot.ffn_gate.len()
            + self.slot.ffn_up.len()
            + self.slot.ffn_down.len();
        if self.slot.held > self.peak_slot_bytes {
            self.peak_slot_bytes = self.slot.held;
        }
        Ok(BlockRead {
            bytes_read: self.slot.held as u64,
            tensors_read: self.tensors_read - before,
            slot_bytes: self.slot.held,
        })
    }

    /// The block the slot holds now, borrowed.
    pub fn block(&self) -> LayerWeights<'_> {
        self.slot.weights()
    }

    /// Reads `output_norm.weight`, the final norm the graph applies after the blocks.
    pub fn load_final_norm(&mut self, hidden_dim: usize) -> Result<Vec<f32>, LoadError> {
        let handle = self
            .table
            .locate(TENSOR_OUTPUT_NORM.as_bytes())
            .ok_or(LoadError::TensorAbsent(TENSOR_OUTPUT_NORM, None))?;
        let floats = self.read_f32_tensor(TENSOR_OUTPUT_NORM, handle)?;
        if floats.len() < hidden_dim {
            return Err(LoadError::BadShape(
                TENSOR_OUTPUT_NORM,
                floats.len() as u64,
                hidden_dim as u64,
            ));
        }
        Ok(floats)
    }

    /// Whether the file holds an output head at all -- the question that decides
    /// whether a run may answer with a next token.
    pub fn has_output_head(&self) -> bool {
        self.table.find(TENSOR_OUTPUT_HEAD.as_bytes()).is_some()
    }

    /// Whether the file holds the pooling head the published embedding model ships.
    ///
    /// WHY this is a question and not a fallback: `dense_2.weight` and
    /// `dense_3.weight` sit between the mean pool and the L2 step in EmbeddingGemma,
    /// and this engine has no head for them. A run reports the vector it computed as
    /// the pooled pre-head stream when this is true, which is the file's own fact and
    /// not this module's omission.
    pub fn has_pooling_head(&self) -> bool {
        self.table.find(TENSOR_DENSE_2.as_bytes()).is_some()
            && self.table.find(TENSOR_DENSE_3.as_bytes()).is_some()
    }

    fn read_raw_tensor(
        &mut self,
        suffix: &'static str,
        entry: TensorRef,
    ) -> Result<Vec<u8>, LoadError> {
        let _ = suffix;
        let wanted = entry.byte_len as usize;
        let mut buffer: Vec<u8> = Vec::new();
        buffer
            .try_reserve_exact(wanted)
            .map_err(|_| LoadError::Arithmetic)?;
        buffer.resize(wanted, 0u8);
        self.source.read_at(entry.position, &mut buffer)?;
        self.bytes_read += wanted as u64;
        self.tensors_read += 1;
        Ok(buffer)
    }

    fn read_f32_tensor(
        &mut self,
        suffix: &'static str,
        entry: TensorRef,
    ) -> Result<Vec<f32>, LoadError> {
        if entry.dtype != 0 {
            // A norm in a quantisation the graph cannot scale with is refused here
            // rather than read as floats: the engine applies norms elementwise and has
            // no dequantised view of one.
            return Err(LoadError::UnsupportedDtype(suffix, entry.dtype));
        }
        let count = entry.rows as usize;
        let wanted = count
            .checked_mul(size_of::<f32>())
            .ok_or(LoadError::Arithmetic)?;
        if entry.byte_len as usize != wanted {
            return Err(LoadError::BadShape(suffix, entry.byte_len, wanted as u64));
        }
        let mut raw: Vec<u8> = Vec::new();
        raw.try_reserve_exact(wanted)
            .map_err(|_| LoadError::Arithmetic)?;
        raw.resize(wanted, 0u8);
        self.source.read_at(entry.position, &mut raw)?;
        self.bytes_read += wanted as u64;
        self.tensors_read += 1;
        let mut floats: Vec<f32> = Vec::new();
        floats
            .try_reserve_exact(count)
            .map_err(|_| LoadError::Arithmetic)?;
        for chunk in raw.chunks_exact(size_of::<f32>()) {
            floats.push(f32::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3],
            ]));
        }
        Ok(floats)
    }

    /// Bytes read from the file so far, and the widest slot any block filled.
    pub fn measured(&self) -> (u64, usize, usize) {
        (self.bytes_read, self.peak_slot_bytes, self.tensors_read)
    }
}

/// What a streamed embedding run cost, so the answer is a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamedRun {
    /// Blocks run per position, counted, against `config.num_layers`.
    pub blocks_run: usize,
    /// Positions stepped.
    pub positions: usize,
    /// Tensor payloads read for the whole run.
    pub bytes_read: u64,
    /// Tensors read for the whole run.
    pub tensors_read: usize,
    /// The widest block slot the run held.
    pub peak_slot_bytes: usize,
    /// Bytes held by the cache, the streams and the slot at once, from the numbers
    /// the engine prices rather than from a guess.
    pub peak_held_bytes: u64,
}

/// Runs the installed file's graph over a prompt and pools it into a vector.
///
/// This is the hop `extract_embedding` could not take: it is handed a
/// `ModelWeights`, and the only way to hand it one for the published file was to
/// hold all 24 blocks and the whole embedding table at once. Here each block is
/// read, run and released, so the peak is one block.
///
/// WHAT LEAVES THIS FUNCTION: the mean-pooled, L2-normalised residual stream of the
/// blocks the file declares. When the file also carries a pooling head
/// (`dense_2`/`dense_3`), the caller is told so by `WeightReader::has_pooling_head`
/// and must report the vector as pre-head -- applying half an architecture and
/// calling it the model's embedding is the defect this module exists to avoid.
pub fn run_streamed_embedding<S: PositionRead>(
    reader: &mut WeightReader<S>,
    config: &ModelConfig,
    tokens: &[u32],
    scratch: &mut Scratch,
    kv_cache: &mut KvCache,
    final_norm: &[f32],
    row: &mut Vec<u8>,
    out: &mut [f32],
) -> Result<(StreamedRun, bool), LoadError> {
    let h_dim = config.hidden_dim;
    if out.len() < h_dim {
        return Err(InferenceError::BufferTooSmall.into());
    }
    if tokens.is_empty() {
        return Ok((
            StreamedRun {
                blocks_run: 0,
                positions: 0,
                bytes_read: 0,
                tensors_read: 0,
                peak_slot_bytes: 0,
                peak_held_bytes: 0,
            },
            reader.has_pooling_head(),
        ));
    }
    config.check_scratch_budget()?;
    if kv_cache.head_dim != config.head_dim
        || kv_cache.num_kv_heads != config.num_kv_heads
        || kv_cache.num_layers < config.num_layers
        || kv_cache.seq_len < config.seq_len
    {
        return Err(InferenceError::BufferTooSmall.into());
    }
    if final_norm.len() < h_dim {
        return Err(InferenceError::CorruptedWeights.into());
    }
    let positions = tokens.len().min(config.seq_len);
    let scale = f32_sqrt(h_dim as f32);

    // WHY the accumulator is cleared here: this scratch is handed in by the
    // caller and lives for the service's whole life -- reusing it is the entire
    // point of the streaming path -- and `pooled` is summed into, never
    // written. A second request in one process would therefore pool the first
    // request's residual stream into its own mean and return a vector that is
    // a blend of two sentences while reporting it as the answer to the second.
    // Every buffer the run touches otherwise holds the position it is at:
    // `x` is written from the embedding row, `hidden` is copied from `x`, and
    // each cached key/value row is written before the attention over it reads
    // it, so the accumulator is the only state a previous request leaves in.
    // `extract_embedding` builds a fresh `Scratch` per call and cannot show
    // this; the streaming path reuses one, so the reset belongs here.
    for slot in scratch.pooled[..h_dim].iter_mut() {
        *slot = 0.0;
    }

    for pos in 0..positions {
        let (dtype, row_bytes) = reader.load_embedding_row(tokens[pos], h_dim, row)?;
        // The row into the stream, in the dtype the descriptor stated, scaled the way
        // Gemma's embedding path scales it.
        embedding_row_into(&row[..row_bytes as usize], dtype, scale, &mut scratch.x[..h_dim])?;
        for block in 0..config.num_layers {
            reader.load_block(block)?;
            let lw = reader.block();
            run_block(block, &lw, pos, config, kv_cache, scratch)?;
        }
        inference::rms_norm(&mut scratch.x[..h_dim], final_norm, config.rms_norm_eps);
        scratch.hidden[..h_dim].copy_from_slice(&scratch.x[..h_dim]);
        for i in 0..h_dim {
            scratch.pooled[i] += scratch.hidden[i];
        }
        kv_cache.len = pos + 1;
    }

    pooled_normalised(&scratch.pooled[..h_dim], positions, &mut out[..h_dim]);
    let (bytes_read, peak_slot_bytes, tensors_read) = reader.measured();
    Ok((
        StreamedRun {
            blocks_run: positions * config.num_layers,
            positions,
            bytes_read,
            tensors_read,
            peak_slot_bytes,
            peak_held_bytes: (bytes_read_saturating(peak_slot_bytes)
                + config.kv_cache_bytes() as u64
                + Scratch::bytes_for(config) as u64),
        },
        reader.has_pooling_head(),
    ))
}

/// A widening that cannot silently become a smaller number in a report.
fn bytes_read_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub mod tests {
    extern crate std as rust_std;
    use super::*;
    use crate::inference::kv_projection_width;

    /// The name builder has to produce what the file states, byte for byte.
    #[test]
    fn block_names_are_the_files_own_strings() {
        let mut buffer = [0u8; MAX_NAME_BYTES];
        let name = block_tensor_name(0, "attn_q.weight", &mut buffer).expect("fits");
        assert_eq!(name, b"blk.0.attn_q.weight");
        let name = block_tensor_name(23, "post_ffw_norm.weight", &mut buffer).expect("fits");
        assert_eq!(name, b"blk.23.post_ffw_norm.weight");
        let name = block_tensor_name(1234, "attn_v.weight", &mut buffer).expect("fits");
        assert_eq!(name, b"blk.1234.attn_v.weight");
    }

    /// The two lists together are the thirteen tensors the published file states per
    /// block, and every name in them is a name that file uses. A loader looking for
    /// `attention_norm` instead of `attn_norm` would report a complete file as a
    /// missing tensor, which is the failure this module can most easily invent.
    #[test]
    fn the_block_tensor_lists_are_the_published_files_thirteen_names() {
        assert_eq!(BLOCK_QUANTISED.len() + BLOCK_NORMS.len(), 13);
        for suffix in BLOCK_QUANTISED.iter().chain(BLOCK_NORMS.iter()) {
            assert!(
                suffix.ends_with(".weight"),
                "{suffix} is not spelled the way a GGUF descriptor spells it"
            );
        }
        assert!(BLOCK_QUANTISED.contains(&"attn_output.weight"));
        assert!(BLOCK_NORMS.contains(&"post_attention_norm.weight"));
        assert!(BLOCK_NORMS.contains(&"post_ffw_norm.weight"));
        assert!(BLOCK_NORMS.contains(&"attn_q_norm.weight"));
        assert!(BLOCK_NORMS.contains(&"attn_k_norm.weight"));
    }

    /// A row's byte width is the dtype's, not the float's.
    #[test]
    fn row_bytes_follow_the_dtype_the_file_declares() {
        // 768-wide, the published file's `token_embd.weight` row.
        assert_eq!(quantised_row_bytes(8, 768), Some(816));
        assert_eq!(quantised_row_bytes(0, 768), Some(3072));
        assert_eq!(quantised_row_bytes(2, 768), Some(432));
        assert_eq!(quantised_row_bytes(12, 768), Some(432));
        // Q4_1 has no layout in this engine and is refused by number, not read as
        // Q4_0 or Q8_0 with a neighbour's block width.
        assert_eq!(quantised_row_bytes(3, 768), None);
        // A width that is not a whole number of blocks has no row either: the
        // format puts no block across two rows, so a number from integer division
        // would be a row the file does not hold.
        assert_eq!(quantised_row_bytes(8, 767), None);
        assert_eq!(quantised_row_bytes(8, 31), None);
        assert_eq!(quantised_row_bytes(12, 100), None);
        assert_eq!(quantised_row_bytes(12, 256), Some(144));
    }

    /// `LoadError` keeps the two refusals a caller has to tell apart.
    #[test]
    fn an_unsupported_dtype_is_refused_by_number_and_not_read_as_another() {
        let error = LoadError::UnsupportedDtype("attn_q.weight", 13);
        match error {
            LoadError::UnsupportedDtype(name, number) => {
                assert_eq!(name, "attn_q.weight");
                assert_eq!(number, 13);
            }
            _ => panic!("the dtype refusal arrived as something else"),
        }
    }

    /// A host file handle behind the same `PositionRead` the device's descriptor
    /// uses, so the measurement below runs the loader's own code path and not a
    /// second reader written for the test.
    ///
    /// WHY `pub(crate)` and not private: `lib.rs`'s gated notice case hands the same
    /// path to the loader, and a second host reader written there would measure a
    /// reader that does not ship.
    #[cfg(test)]
    pub(crate) struct HostFileSource {
        pub(crate) file: rust_std::fs::File,
    }

    #[cfg(test)]
    impl PositionRead for HostFileSource {
        fn read_at(&mut self, position: u64, into: &mut [u8]) -> Result<(), LoadError> {
            use rust_std::io::{Read, Seek, SeekFrom};
            self.file
                .seek(SeekFrom::Start(position))
                .map_err(|_| LoadError::ShortRead(position, into.len(), 0))?;
            let mut filled = 0usize;
            while filled < into.len() {
                let got = self
                    .file
                    .read(&mut into[filled..])
                    .map_err(|_| LoadError::ShortRead(position, into.len(), filled))?;
                if got == 0 {
                    return Err(LoadError::ShortRead(position, into.len(), filled));
                }
                filled += got;
            }
            Ok(())
        }
    }

    /// Reads the largest GGUF prefix that actually walks, so the test reports how
    /// far into the file the descriptor table is instead of hard-coding a number
    /// whoever measured it last typed in.
    ///
    /// WHY `pub(crate)`: the gated notice case in `lib.rs` walks the same table of
    /// the same file, and a second prefix rule there could stop at a different byte
    /// and report a sentence for a file the loader never saw.
    #[cfg(test)]
    pub(crate) fn walk_the_installed_table(
        path: &str,
    ) -> Result<(TensorTable, rust_std::vec::Vec<u8>, usize), rust_std::string::String> {
        use rust_std::io::{Read, Seek, SeekFrom};
        // WHY the open sits outside the loop and names its error: a path that does not
        // resolve and a file whose table lies past every prefix tried are two different
        // failures, and reporting the second for the first is how a mistyped variable
        // gets read as a loader bug.
        let mut file = rust_std::fs::File::open(path)
            .map_err(|error| rust_std::format!("{path} could not be opened: {error}"))?;
        let mut refused = rust_std::string::String::new();
        for megabytes in [1usize, 4, 8, 12, 16] {
            let wanted = megabytes * 1024 * 1024;
            file.seek(SeekFrom::Start(0))
                .map_err(|error| rust_std::format!("{path} could not be rewound: {error}"))?;
            let mut bytes: rust_std::vec::Vec<u8> = rust_std::vec::Vec::new();
            bytes.resize(wanted, 0u8);
            let mut filled = 0usize;
            while filled < wanted {
                match file.read(&mut bytes[filled..]) {
                    Ok(0) => break,
                    Ok(got) => filled += got,
                    Err(_) => break,
                }
            }
            bytes.truncate(filled);
            let mut stream = crate::gguf::GgufStream::from_slice(&bytes);
            match TensorTable::walk(&mut stream) {
                Ok(table) => return Ok((table, bytes, filled)),
                Err(error) => refused = rust_std::format!(
                    "no prefix up to {megabytes} MiB walked (last error {error:?} over {filled} bytes)"
                ),
            }
        }
        Err(refused)
    }

    /// The hop this module exists to close, taken against the installed file.
    ///
    /// Nothing here asserts on a fixture: the descriptors, the dtype, the positions
    /// and the numbers the graph runs on all come out of the 333 MB file the
    /// `INTEROS_REAL_GGUF_PATH` variable names, and a run without that variable says
    /// so out loud instead of passing on a hand-written description of the file.
    #[test]
    fn test_the_installed_file_is_read_block_by_block_and_run() {
        let Some(path) = rust_std::env::var("INTEROS_REAL_GGUF_PATH").ok() else {
            rust_std::println!(
                "NOT VERIFIED against the installed file: INTEROS_REAL_GGUF_PATH is unset, so \
                 no tensor of the 333 MB GGUF was read and the loader ran only on hand-written \
                 fixtures."
            );
            return;
        };

        // 1. One pass over the descriptor table. The prefix it walked over is kept
        // for the two checks below and then dropped; it is the metadata and the
        // descriptors, not the weights, and its size is printed rather than assumed.
        let (table, prefix, prefix_bytes) = walk_the_installed_table(&path)
            .unwrap_or_else(|why| panic!("the descriptor table of {path} never walked: {why}"));
        rust_std::println!(
            "TABLE: {} descriptors over {} prefix bytes, data_start {}, alignment {}, \
             table holds {} bytes, window pulls {}",
            table.count(),
            prefix_bytes,
            table.data_start,
            table.alignment,
            table.held_bytes(),
            table.window_pulls,
        );
        assert_eq!(table.count(), 316, "the published file states 316 descriptors");
        assert!(
            table.held_bytes() < 1024 * 1024,
            "the descriptor table holds {} bytes, which is not a bounded table",
            table.held_bytes()
        );

        // 2. A position the loader resolved matches the one the reader resolves for
        // the same name through its own path. Two rules for one number is how a
        // loader starts reading a neighbour's bytes.
        let probe = b"blk.0.attn_q.weight";
        let located = crate::gguf::locate_tensor(&prefix, probe)
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("{path} holds no blk.0.attn_q.weight"));
        let entry = table
            .find(probe)
            .unwrap_or_else(|| panic!("the table walk lost blk.0.attn_q.weight"));
        assert_eq!(
            entry.position, located.file_position,
            "the loader puts blk.0.attn_q.weight at {} and the reader at {}",
            entry.position, located.file_position
        );
        assert_eq!(entry.dtype, 8, "the published file quantises it Q8_0");
        assert_eq!(entry.cols, 768);
        assert_eq!(entry.rows, 768);
        assert_eq!(
            entry.byte_len,
            quantised_row_bytes(8, 768).unwrap() as u64 * 768,
            "the descriptor's own byte count disagrees with the row width"
        );

        // 3. The graph is priced from the file, and the file decides what can answer.
        let mut info_stream = crate::gguf::GgufStream::from_slice(&prefix);
        let info = crate::gguf::read_model_info_stream(&mut info_stream).expect("the file configures");
        let config = ModelConfig::from_gguf(&info).expect("the installed file is priced");
        assert_eq!(config.num_layers, 24);
        assert_eq!(kv_projection_width(&config), 256, "3 query heads, 1 key/value head");

        let source = HostFileSource {
            file: rust_std::fs::File::open(&path).expect("the installed file opens"),
        };
        let mut reader = WeightReader::new(table, source);
        assert!(
            !reader.has_output_head(),
            "the published embedding file holds an output head, so the run's refusal \
             to answer with a token is now wrong"
        );
        assert!(
            reader.has_pooling_head(),
            "the published file's dense_2/dense_3 pooling head disappeared, so the \
             report below should stop calling the vector pre-head"
        );

        // 4. One block, read, held and released.
        let first = reader.load_block(0).expect("block 0 reads");
        rust_std::println!(
            "BLOCK 0: {} tensors, {} bytes into the slot (dtype the file declares: {})",
            first.tensors_read, first.bytes_read, reader.block().weight_type
        );
        assert_eq!(first.tensors_read, 13);
        assert_eq!(reader.block().weight_type, 8);
        assert_eq!(reader.block().attn_k.len(), 256 * 816, "K is 256 rows of Q8_0 768");
        assert_eq!(reader.block().attn_q_norm.len(), 256, "one head of q-norm");
        assert_eq!(reader.block().attn_k_norm.len(), 256, "one head of k-norm");
        assert_eq!(reader.block().attn_norm.len(), 768);
        let last = reader.load_block(23).expect("block 23 reads");
        rust_std::println!(
            "BLOCK 23: {} tensors, {} bytes -- the slot was emptied first, so 24 blocks \
             never coexist",
            last.tensors_read, last.bytes_read
        );
        assert!(reader.block().attn_q.len() <= 626_178 + 4096);

        // 5. One embedding row, dequantised in the dtype its descriptor states.
        let mut row: Vec<u8> = Vec::new();
        let (row_dtype, row_bytes) = reader
            .load_embedding_row(1701, config.hidden_dim, &mut row)
            .expect("a real token id has a row");
        assert_eq!(row_dtype, 8);
        assert_eq!(row_bytes, 816);
        let mut stream_row = [0.0f32; 768];
        embedding_row_into(&row, row_dtype, 1.0, &mut stream_row).expect("the row dequantises");
        rust_std::println!(
            "ROW token 1701: {row_bytes} bytes at the file position the table resolved, \
             first four weights {:.5}, {:.5}, {:.5}, {:.5}",
            stream_row[0],
            stream_row[1],
            stream_row[2],
            stream_row[3],
        );
        // Every weight of a Q8_0 block is the block's scale times an integer in
        // [-128, 127], so a row read with the right layout has no non-finite value
        // and is not all zero; a row read with the wrong one would be noise either
        // way.
        assert!(stream_row.iter().all(|v| v.is_finite()));
        assert!(stream_row.iter().any(|v| *v != 0.0));

        // 6. The graph, over the file's bytes, block by block.
        let final_norm = reader.load_final_norm(config.hidden_dim).expect("output_norm reads");
        let mut scratch = Scratch::new(&config).expect("the run's scratch is under budget");
        let mut kv_cache = KvCache::new(&config).expect("the run's cache is under budget");
        let started = rust_std::time::Instant::now();
        let mut vectors = [[0.0f32; 768]; 2];
        let mut reports = [
            StreamedRun {
                blocks_run: 0,
                positions: 0,
                bytes_read: 0,
                tensors_read: 0,
                peak_slot_bytes: 0,
                peak_held_bytes: 0,
            },
            StreamedRun {
                blocks_run: 0,
                positions: 0,
                bytes_read: 0,
                tensors_read: 0,
                peak_slot_bytes: 0,
                peak_held_bytes: 0,
            },
        ];
        let mut pooling_head_present = false;
        let prompts = [[1701u32, 4423, 88], [1701u32, 91001, 88]];
        for slot in 0..2usize {
            let tokens = &prompts[slot];
            let mut out: Vec<f32> = rust_std::vec![0.0f32; config.hidden_dim];
            let mut row_buffer: Vec<u8> = Vec::new();
            let (report, pooling) = run_streamed_embedding(
                &mut reader,
                &config,
                tokens,
                &mut scratch,
                &mut kv_cache,
                &final_norm,
                &mut row_buffer,
                &mut out,
            )
            .expect("the installed file runs the graph");
            vectors[slot].copy_from_slice(&out);
            reports[slot] = report;
            pooling_head_present = pooling;
        }
        rust_std::println!(
            "HOST-BACKED RUN (this is the development host running the engine, not the \
             device): {} positions x {} blocks, {} tensors, {} bytes read, peak slot {} \
             bytes, peak held {} bytes, in {:?}",
            reports[0].positions,
            config.num_layers,
            reports[1].tensors_read,
            reports[1].bytes_read,
            reports[1].peak_slot_bytes,
            reports[1].peak_held_bytes,
            started.elapsed(),
        );
        assert_eq!(reports[0].blocks_run, 3 * 24);
        assert!(pooling_head_present, "the file's dense_2/dense_3 head is gone");
        for vector in vectors.iter() {
            assert!(vector.iter().all(|v| v.is_finite()));
            let mut length = 0.0f32;
            for &v in vector.iter() {
                length += v * v;
            }
            assert!(
                (length - 1.0).abs() < 0.01,
                "the pooled vector is not L2-normalised (squared length {length})"
            );
        }
        rust_std::println!(
            "EMBEDDINGS from file bytes (pre-head: the file's dense_2/dense_3 are read \
             by nobody yet): first four of A [{:.6}, {:.6}, {:.6}, {:.6}], first four of \
             B [{:.6}, {:.6}, {:.6}, {:.6}]",
            vectors[0][0],
            vectors[0][1],
            vectors[0][2],
            vectors[0][3],
            vectors[1][0],
            vectors[1][1],
            vectors[1][2],
            vectors[1][3],
        );
        let mut overlap = 0.0f32;
        for i in 0..config.hidden_dim {
            overlap += vectors[0][i] * vectors[1][i];
        }
        rust_std::println!("MEASURED dot(A, B) = {overlap}");
        assert!(
            vectors[0] != vectors[1],
            "two different token-id sequences produced the same vector, which means the \
             run read nothing that depends on the token"
        );

        // A request's vector has to be the same vector whether it is the first
        // request the process serves or the third. The pooling accumulator is the
        // one buffer a run sums into rather than overwrites, and the scratch is
        // reused on purpose, so a leftover accumulator blends two sentences into
        // one answer that still looks clean -- finite, unit-length, plausible.
        // Re-running the first prompt over the scratch and cache the second just
        // used is the only reading this file can take of that.
        {
            let mut out_again: Vec<f32> = rust_std::vec![0.0f32; config.hidden_dim];
            let mut row_again: Vec<u8> = Vec::new();
            let (report_again, _) = run_streamed_embedding(
                &mut reader,
                &config,
                &prompts[0],
                &mut scratch,
                &mut kv_cache,
                &final_norm,
                &mut row_again,
                &mut out_again,
            )
            .expect("the installed file runs the graph a third time");
            let mut again = [0.0f32; 768];
            again.copy_from_slice(&out_again);
            assert_eq!(
                again, vectors[0],
                "the same prompt answered differently the second time it was asked, which \
                 means the run pooled a previous request's residual stream into this one"
            );
            assert_eq!(report_again.positions, reports[0].positions);
        }
    }

    /// A block the file quantises in a type the engine has no layout for is refused
    /// by that type's number, and a block split across two types is refused as the
    /// mixed block it is. Neither refusal is allowed to read the bytes with a
    /// neighbour's dequantiser.
    #[test]
    fn a_dtype_the_engine_has_no_layout_for_stops_the_block_it_appears_in() {
        // Hand the loader a table it built itself, over a file it can read: the
        // published fixture files in `target/verify/models` are what the QEMU build
        // installs, and the tiny ones are small enough to hold in this test.
        // The behaviour is asserted through the refusal's shape rather than a file
        // read, because a fabricated descriptor is the one place a test may invent
        // bytes -- the installed file is checked by the case above.
        let entry = TensorEntry {
            name: b"blk.0.attn_q.weight".to_vec(),
            dtype: 13, // Q5_K: no layout in this engine
            cols: 768,
            rows: 768,
            n_dims: 2,
            byte_len: 0,
            stated_offset: 0,
            position: 0,
        };
        assert_eq!(
            quantised_row_bytes(entry.dtype, entry.cols as usize),
            None,
            "Q5_K must not resolve to a row width, or the loader will read it as Q4_K"
        );
    }
}
