// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Retrieval-Augmented Generation (RAG) pipeline for InterOS Core: document
//! chunking, vector indexing, cosine ranking and prompt injection. The vectors it
//! indexes are the lexical projection of `crate::embedding`, and this pipeline
//! answers with the chunks it stored rather than with text a model produced, so no
//! part of it is a neural language model.
//!
//! Strictly `#![no_std]` and zero heap allocation in all execution and query loops.
//!
//! Key components:
//! 1. Document discovery & ingestion from `/storage/` (e.g. `/storage/downloads/`, `/storage/notes/`).
//! 2. Document parsing & chunking with stable 64-bit deterministic chunk IDs, exact character
//!    bounds (`start_offset`, `end_offset`), and source attribution metadata.
//! 3. Vector indexing of the projection in `crate::embedding` -- the deterministic
//!    lexical n-gram feature hash that module documents as NOT physical neural
//!    EmbeddingGemma 2 weights -- stored here at 256 dimensions of Q7 fixed-point.
//!    The hash will fill 128, 256, 512 or 768 slots for a caller that asks, but
//!    this index holds 256 and compares no wider (see `RAG_VECTOR_DIM`).
//! 4. Zero-allocation cosine similarity ranking algorithm returning top-K matches with
//!    exact filename, chunk offset, confidence percentage, and neural vs lexical attribution.
//! 5. Exact-attribution prompt injection for RAG queries without hallucinating or fabricating citations.
//! 6. Safe index persistence to `/data/search/vector.idx` or `/storage/vector.idx`.

use interos_std as std;
use crate::embedding::compute_embedding_768;

/// Maximum byte length for a source document path string.
pub const MAX_DOC_PATH_LEN: usize = 64;

/// Maximum byte length for a chunk text snippet.
pub const MAX_CHUNK_TEXT_LEN: usize = 192;

/// The width one chunk's stored vector occupies: the first 256 dimensions of
/// `crate::embedding`'s projection, held as Q7 fixed-point.
///
/// WHY the number is a ceiling and not a starting point: `search` clamps the
/// dimension a caller asks for into `128..=RAG_VECTOR_DIM`, a stored record is a
/// stack array of exactly this width, and nothing in the index can compare a
/// dimension it has not kept. Asking for 512 or 768 is therefore answered from
/// these 256 -- it is not a wider vector, and this type does not have one.
pub const RAG_VECTOR_DIM: usize = 256;

/// Maximum number of indexed document chunks held in memory without heap allocation.
pub const MAX_RAG_CHUNKS: usize = 48;

/// Binary index format magic: `0x5241_4758` (ASCII `b"RAGX"`).
pub const RAG_INDEX_MAGIC: u32 = 0x5241_4758;

/// Binary index format version.
pub const RAG_INDEX_VERSION: u32 = 1;

/// Primary storage path for persistent vector index.
pub const PATH_SEARCH_INDEX_DATA: &str = "/data/search/vector.idx";

/// Fallback storage path for persistent vector index.
pub const PATH_SEARCH_INDEX_STORAGE: &str = "/storage/vector.idx";

/// Default directories to discover and ingest documents from.
pub const STORAGE_SEARCH_DIRS: &[&str] = &[
    "/storage/notes",
    "/storage/downloads",
    "/storage",
];

/// A single parsed document chunk with stable ID and source attribution.
#[derive(Clone, Copy)]
pub struct RagChunk {
    /// Deterministic 64-bit chunk ID based on document path, chunk index, and offset.
    pub id: u64,
    /// Exact source document path (e.g. `"/storage/notes/spec.txt"`).
    pub source_path: [u8; MAX_DOC_PATH_LEN],
    pub source_path_len: u8,
    /// Starting character / byte offset in source document.
    pub start_offset: u32,
    /// Ending character / byte offset in source document.
    pub end_offset: u32,
    /// Chunk sequence index within the document.
    pub chunk_index: u16,
    /// Raw text snippet of this chunk.
    pub text: [u8; MAX_CHUNK_TEXT_LEN],
    pub text_len: u16,
    /// 256-D Q7 fixed-point vector embedding (Matryoshka representation).
    pub embedding: [i8; RAG_VECTOR_DIM],
}

impl Default for RagChunk {
    fn default() -> Self {
        Self {
            id: 0,
            source_path: [0u8; MAX_DOC_PATH_LEN],
            source_path_len: 0,
            start_offset: 0,
            end_offset: 0,
            chunk_index: 0,
            text: [0u8; MAX_CHUNK_TEXT_LEN],
            text_len: 0,
            embedding: [0i8; RAG_VECTOR_DIM],
        }
    }
}

impl RagChunk {
    pub fn source_path_str(&self) -> &str {
        core::str::from_utf8(&self.source_path[..self.source_path_len as usize]).unwrap_or("")
    }

    pub fn text_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.text_len as usize]).unwrap_or("")
    }
}

/// A ranked RAG search hit with similarity score and exact citation metadata.
#[derive(Debug, Clone, Copy)]
pub struct RagMatch {
    /// Stable 64-bit chunk ID.
    pub chunk_id: u64,
    /// Exact source document path.
    pub source_path: [u8; MAX_DOC_PATH_LEN],
    pub source_path_len: u8,
    /// Starting character / byte offset in source document.
    pub start_offset: u32,
    /// Ending character / byte offset in source document.
    pub end_offset: u32,
    /// Cosine similarity score in Q15 format (-32768 to 32767).
    pub score_q15: i16,
    /// Normalized confidence percentage (0 to 100%).
    pub confidence_percent: u8,
    /// Platform Truth Contract: whether generated with physical neural weights vs lexical baseline.
    pub is_neural: bool,
    /// Text snippet from the matched chunk.
    pub snippet: [u8; MAX_CHUNK_TEXT_LEN],
    pub snippet_len: u16,
}

impl Default for RagMatch {
    fn default() -> Self {
        Self {
            chunk_id: 0,
            source_path: [0u8; MAX_DOC_PATH_LEN],
            source_path_len: 0,
            start_offset: 0,
            end_offset: 0,
            score_q15: 0,
            confidence_percent: 0,
            is_neural: false,
            snippet: [0u8; MAX_CHUNK_TEXT_LEN],
            snippet_len: 0,
        }
    }
}

impl RagMatch {
    pub fn source_path_str(&self) -> &str {
        core::str::from_utf8(&self.source_path[..self.source_path_len as usize]).unwrap_or("")
    }

    pub fn snippet_str(&self) -> &str {
        core::str::from_utf8(&self.snippet[..self.snippet_len as usize]).unwrap_or("")
    }
}

/// Computes a stable, deterministic 64-bit ID for a document chunk.
pub fn compute_chunk_id(source_path: &[u8], chunk_index: u16, start_offset: u32) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in source_path {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= chunk_index as u64;
    h = h.wrapping_mul(0x100000001b3);
    h ^= start_offset as u64;
    h = h.wrapping_mul(0x100000001b3);
    // SplitMix64 finalizer
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h
}

/// In-memory bounded vector index for on-device RAG document retrieval.
pub struct RagIndex {
    pub chunks: [RagChunk; MAX_RAG_CHUNKS],
    pub count: usize,
    /// Platform Truth Contract: true if neural EmbeddingGemma 2 weights are present;
    /// false if operating in deterministic lexical baseline mode.
    pub is_neural: bool,
}

impl RagIndex {
    /// Creates an empty vector index with specified neural weight verification status.
    pub const fn new(is_neural: bool) -> Self {
        Self {
            chunks: [RagChunk {
                id: 0,
                source_path: [0u8; MAX_DOC_PATH_LEN],
                source_path_len: 0,
                start_offset: 0,
                end_offset: 0,
                chunk_index: 0,
                text: [0u8; MAX_CHUNK_TEXT_LEN],
                text_len: 0,
                embedding: [0i8; RAG_VECTOR_DIM],
            }; MAX_RAG_CHUNKS],
            count: 0,
            is_neural,
        }
    }

    /// Clears all indexed chunks.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Adds a pre-formed chunk to the index if space remains.
    pub fn add_chunk(&mut self, chunk: RagChunk) -> bool {
        if self.count >= MAX_RAG_CHUNKS {
            return false;
        }
        self.chunks[self.count] = chunk;
        self.count += 1;
        true
    }

    /// Parses and ingests a document by splitting into bounded chunks, generating
    /// Matryoshka vector embeddings, and indexing them with stable IDs and offsets.
    ///
    /// Returns the number of chunks added.
    pub fn ingest_document(&mut self, path: &[u8], content: &[u8]) -> usize {
        if content.is_empty() || path.is_empty() {
            return 0;
        }

        let mut chunks_added = 0usize;
        let mut offset = 0usize;
        let mut chunk_idx = 0u16;

        let path_len = path.len().min(MAX_DOC_PATH_LEN);

        while offset < content.len() && self.count < MAX_RAG_CHUNKS {
            let remaining = content.len() - offset;
            let target_chunk_len = 140usize.min(MAX_CHUNK_TEXT_LEN).min(remaining);

            // Find a natural break (newline or period or space) near target_chunk_len
            let mut actual_len = target_chunk_len;
            if remaining > target_chunk_len {
                let search_window_start = (target_chunk_len.saturating_sub(40)).max(1);
                let window = &content[offset + search_window_start..offset + target_chunk_len];
                if let Some(pos) = window.iter().rposition(|&b| b == b'\n' || b == b'.' || b == b' ') {
                    actual_len = search_window_start + pos + 1;
                }
            }

            let chunk_slice = &content[offset..offset + actual_len];

            let mut chunk = RagChunk::default();
            chunk.source_path[..path_len].copy_from_slice(&path[..path_len]);
            chunk.source_path_len = path_len as u8;
            chunk.start_offset = offset as u32;
            chunk.end_offset = (offset + actual_len) as u32;
            chunk.chunk_index = chunk_idx;
            chunk.id = compute_chunk_id(path, chunk_idx, chunk.start_offset);

            let text_copy_len = actual_len.min(MAX_CHUNK_TEXT_LEN);
            chunk.text[..text_copy_len].copy_from_slice(&chunk_slice[..text_copy_len]);
            chunk.text_len = text_copy_len as u16;

            // The chunk's text projected onto the 256 dimensions this index stores:
            // `crate::embedding`'s deterministic lexical n-gram hash, computed on the
            // stack. Not the installed model's weights, and not a wider vector.
            compute_embedding_768(
                &chunk.text[..chunk.text_len as usize],
                RAG_VECTOR_DIM,
                None,
                Some(&mut chunk.embedding),
            );

            self.chunks[self.count] = chunk;
            self.count += 1;
            chunks_added += 1;

            offset += actual_len;
            chunk_idx = chunk_idx.saturating_add(1);
        }

        chunks_added
    }

    /// Performs cosine similarity ranking across all indexed chunks for a query.
    ///
    /// The caller's `target_dim` is clamped into `128..=RAG_VECTOR_DIM`, so a
    /// request for 512 or 768 compares the 256 dimensions the index holds and
    /// returns that score -- not an error, and not a wider vector.
    /// Returns the number of matches written into `out_matches` (up to `top_k`).
    pub fn search(
        &self,
        query: &[u8],
        target_dim: usize,
        top_k: usize,
        min_confidence: u8,
        out_matches: &mut [RagMatch],
    ) -> usize {
        if query.is_empty() || self.count == 0 || out_matches.is_empty() || top_k == 0 {
            return 0;
        }

        let dim = target_dim.clamp(128, RAG_VECTOR_DIM);

        // Compute query embedding vector on the stack
        let mut query_emb = [0i8; RAG_VECTOR_DIM];
        compute_embedding_768(query, dim, None, Some(&mut query_emb));

        let max_results = top_k.min(out_matches.len());
        let mut found_count = 0usize;

        for i in 0..self.count {
            let chunk = &self.chunks[i];

            // Compute cosine similarity between unit-normalized Q7 vectors
            let score_q15 = std::ai::cosine_similarity_q7(
                &query_emb[..dim],
                &chunk.embedding[..dim],
            );

            // Convert Q15 (-32768..32767) to positive confidence percentage (0..100%)
            let confidence = if score_q15 <= 0 {
                0u8
            } else {
                ((score_q15 as u32 * 100) / 32767).min(100) as u8
            };

            if confidence < min_confidence {
                continue;
            }

            let item = RagMatch {
                chunk_id: chunk.id,
                source_path: chunk.source_path,
                source_path_len: chunk.source_path_len,
                start_offset: chunk.start_offset,
                end_offset: chunk.end_offset,
                score_q15,
                confidence_percent: confidence,
                is_neural: self.is_neural,
                snippet: chunk.text,
                snippet_len: chunk.text_len,
            };

            // Insertion sort into top-K results (descending score)
            let mut insert_pos = found_count;
            while insert_pos > 0 && out_matches[insert_pos - 1].score_q15 < score_q15 {
                if insert_pos < max_results {
                    out_matches[insert_pos] = out_matches[insert_pos - 1];
                }
                insert_pos -= 1;
            }

            if insert_pos < max_results {
                out_matches[insert_pos] = item;
                if found_count < max_results {
                    found_count += 1;
                }
            }
        }

        found_count
    }

    /// Discovers text files in `/storage/` directories and ingests them into the vector index.
    ///
    /// Checks `/storage/notes`, `/storage/downloads`, and `/storage`.
    /// Returns the total number of document chunks indexed.
    pub fn discover_and_ingest_storage(&mut self) -> usize {
        let mut list_buf = [0u8; 1024];
        let mut file_buf = [0u8; 2048];
        let mut total_chunks = 0usize;

        for &dir in STORAGE_SEARCH_DIRS {
            if self.count >= MAX_RAG_CHUNKS {
                break;
            }

            if let Ok(bytes_read) = std::fs_list(dir, &mut list_buf) {
                let listing = &list_buf[..bytes_read.min(list_buf.len())];
                for line in listing.split(|&b| b == b'\n') {
                    if line.is_empty() || self.count >= MAX_RAG_CHUNKS {
                        continue;
                    }
                    // Parse filename (first field before delimiter)
                    let fname_end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
                    let fname = &line[..fname_end];

                    // Check if filename suggests a readable document (.txt, .md, note, doc)
                    let is_doc = fname.ends_with(b".txt")
                        || fname.ends_with(b".md")
                        || fname.ends_with(b".log")
                        || fname.ends_with(b".json")
                        || fname.windows(4).any(|w| w.eq_ignore_ascii_case(b"note"));

                    if !is_doc {
                        continue;
                    }

                    // Format full path
                    let mut full_path = [0u8; MAX_DOC_PATH_LEN];
                    let dir_b = dir.as_bytes();
                    if dir_b.len() + 1 + fname.len() >= MAX_DOC_PATH_LEN {
                        continue;
                    }
                    full_path[..dir_b.len()].copy_from_slice(dir_b);
                    full_path[dir_b.len()] = b'/';
                    let fname_start = dir_b.len() + 1;
                    let fname_end_idx = fname_start + fname.len();
                    full_path[fname_start..fname_end_idx].copy_from_slice(fname);

                    if let Ok(full_path_str) = core::str::from_utf8(&full_path[..fname_end_idx]) {
                        if let Ok(fd) = std::fs_open(full_path_str, std::FS_READ) {
                            if let Ok(read_len) = std::fs_read(fd, &mut file_buf) {
                                let _ = std::fs_close(fd);
                                if read_len > 0 {
                                    let added = self.ingest_document(
                                        &full_path[..fname_end_idx],
                                        &file_buf[..read_len.min(file_buf.len())],
                                    );
                                    total_chunks += added;
                                }
                            } else {
                                let _ = std::fs_close(fd);
                            }
                        }
                    }
                }
            }
        }

        total_chunks
    }

    /// Serializes and writes the vector index safely to a file path.
    pub fn save_to_disk(&self, path: &str) -> Result<(), ()> {
        let fd = std::fs_open(path, std::FS_WRITE).map_err(|_| ())?;

        // 1. Write header: magic (4), version (4), count (4), dim (4)
        let mut hdr = [0u8; 16];
        hdr[0..4].copy_from_slice(&RAG_INDEX_MAGIC.to_le_bytes());
        hdr[4..8].copy_from_slice(&RAG_INDEX_VERSION.to_le_bytes());
        hdr[8..12].copy_from_slice(&(self.count as u32).to_le_bytes());
        hdr[12..16].copy_from_slice(&(RAG_VECTOR_DIM as u32).to_le_bytes());
        std::fs_write(fd, &hdr).map_err(|_| {
            let _ = std::fs_close(fd);
            ()
        })?;

        // 2. Write each chunk
        for i in 0..self.count {
            let chunk = &self.chunks[i];
            let chunk_bytes = unsafe {
                core::slice::from_raw_parts(
                    chunk as *const _ as *const u8,
                    core::mem::size_of::<RagChunk>(),
                )
            };
            if std::fs_write(fd, chunk_bytes).is_err() {
                let _ = std::fs_close(fd);
                return Err(());
            }
        }

        let _ = std::fs_close(fd);
        Ok(())
    }

    /// Loads and verifies a vector index from disk.
    pub fn load_from_disk(&mut self, path: &str) -> Result<usize, ()> {
        let fd = std::fs_open(path, std::FS_READ).map_err(|_| ())?;

        let mut hdr = [0u8; 16];
        let n = std::fs_read(fd, &mut hdr).map_err(|_| {
            let _ = std::fs_close(fd);
            ()
        })?;
        if n < 16 {
            let _ = std::fs_close(fd);
            return Err(());
        }

        let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let version = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let count = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
        let dim = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;

        if magic != RAG_INDEX_MAGIC || version != RAG_INDEX_VERSION || dim != RAG_VECTOR_DIM {
            let _ = std::fs_close(fd);
            return Err(());
        }

        self.clear();
        let load_count = count.min(MAX_RAG_CHUNKS);

        let mut chunk_buf = [0u8; core::mem::size_of::<RagChunk>()];
        for _ in 0..load_count {
            let read_bytes = std::fs_read(fd, &mut chunk_buf).map_err(|_| {
                let _ = std::fs_close(fd);
                ()
            })?;
            if read_bytes != core::mem::size_of::<RagChunk>() {
                break;
            }
            let chunk = unsafe {
                core::ptr::read_unaligned(chunk_buf.as_ptr() as *const RagChunk)
            };
            self.add_chunk(chunk);
        }

        let _ = std::fs_close(fd);
        Ok(self.count)
    }
}

/// Formats retrieved document context and query into a RAG generative prompt with exact citations.
///
/// Guaranteed zero heap allocation. Respects the Platform Truth Contract:
/// - Exact filename, character offsets, and confidence score are rendered.
/// - When no matching documents are retrieved, the prompt explicitly indicates
///   absence of context rather than hallucinating or fabricating citations.
pub fn format_rag_prompt(
    query: &[u8],
    matches: &[RagMatch],
    out_buf: &mut [u8],
) -> Result<usize, ()> {
    let mut cursor = 0usize;

    macro_rules! append {
        ($bytes:expr) => {{
            let b: &[u8] = $bytes;
            if cursor + b.len() > out_buf.len() {
                return Err(());
            }
            out_buf[cursor..cursor + b.len()].copy_from_slice(b);
            cursor += b.len();
        }};
    }

    append!(b"--- RETRIEVAL CONTEXT ---\n");

    if matches.is_empty() {
        append!(b"[No matching local documents retrieved from storage]\n\n");
    } else {
        append!(b"Retrieved verified local documents:\n");
        for (i, m) in matches.iter().enumerate() {
            append!(b"[");
            let mut num_buf = [0u8; 4];
            let num_len = format_u32((i + 1) as u32, &mut num_buf);
            append!(&num_buf[..num_len]);
            append!(b"] Source: ");
            append!(&m.source_path[..m.source_path_len as usize]);
            append!(b" (offset: ");

            let mut off_buf = [0u8; 10];
            let s_len = format_u32(m.start_offset, &mut off_buf);
            append!(&off_buf[..s_len]);
            append!(b"-");
            let e_len = format_u32(m.end_offset, &mut off_buf);
            append!(&off_buf[..e_len]);

            append!(b", confidence: ");
            let c_len = format_u32(m.confidence_percent as u32, &mut off_buf);
            append!(&off_buf[..c_len]);
            append!(b"%, mode: ");
            if m.is_neural {
                append!(b"neural");
            } else {
                append!(b"lexical");
            }
            append!(b")\nSnippet: \"");
            append!(&m.snippet[..m.snippet_len as usize]);
            append!(b"\"\n\n");
        }
    }

    append!(b"--- USER QUERY ---\n");
    append!(query);
    append!(b"\n\n--- INSTRUCTIONS ---\n");
    if matches.is_empty() {
        append!(b"Answer truthfully. Report that no relevant local document context was found in storage.\n");
    } else {
        append!(b"Answer the query based ONLY on the verified local document context above. Cite the exact source path and offsets.\n");
    }

    Ok(cursor)
}

/// Helper to format a u32 into an ASCII buffer without heap allocations.
pub(crate) fn format_u32(mut val: u32, buf: &mut [u8]) -> usize {
    if val == 0 {
        if !buf.is_empty() {
            buf[0] = b'0';
            return 1;
        }
        return 0;
    }
    let mut temp = [0u8; 10];
    let mut i = 0usize;
    while val > 0 && i < 10 {
        temp[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }
    let len = i;
    for j in 0..len {
        if j < buf.len() {
            buf[j] = temp[len - 1 - j];
        }
    }
    len.min(buf.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_chunk_id_stability() {
        let path = b"/storage/notes/kernel_architecture.txt";
        let id1 = compute_chunk_id(path, 0, 0);
        let id2 = compute_chunk_id(path, 0, 0);
        assert_eq!(id1, id2, "chunk ID must be stable and deterministic");

        let id3 = compute_chunk_id(path, 1, 140);
        assert_ne!(id1, id3, "different chunk index/offset must yield distinct IDs");
    }

    #[test]
    fn test_ingest_document_and_chunking() {
        let mut index = RagIndex::new(false);
        let path = b"/storage/notes/meeting.txt";
        let doc = b"InterOS microkernel architecture meeting notes.\n\
We discussed capability security boundaries and zero ambient authority.\n\
All services must be granted explicit CapHandles for hardware and IPC.\n\
Vector search is enabled for notes and downloads.";

        let added = index.ingest_document(path, doc);
        assert!(added >= 1, "at least one chunk must be ingested");
        assert_eq!(index.count, added);

        let first = &index.chunks[0];
        assert_eq!(&first.source_path[..first.source_path_len as usize], path);
        assert_eq!(first.start_offset, 0);
        assert!(first.end_offset > 0);
        assert!(first.text_len > 0);
    }

    #[test]
    fn test_search_and_cosine_ranking() {
        let mut index = RagIndex::new(false);
        index.ingest_document(
            b"/storage/notes/wifi.txt",
            b"Wi-Fi wireless network settings configuration WPA3 security password.",
        );
        index.ingest_document(
            b"/storage/notes/cooking.txt",
            b"Delicious strawberry shortcake baking recipe flour butter sugar oven temperature.",
        );

        let mut matches = [RagMatch::default(); 4];
        let count = index.search(b"wireless network setup", 256, 4, 10, &mut matches);

        assert!(count >= 1, "expected at least 1 match for wireless network");
        assert_eq!(
            &matches[0].source_path[..matches[0].source_path_len as usize],
            b"/storage/notes/wifi.txt"
        );
        assert!(matches[0].confidence_percent >= 10);
    }

    #[test]
    fn test_matryoshka_dimension_subvector_search() {
        let mut index = RagIndex::new(false);
        index.ingest_document(
            b"/storage/notes/power.txt",
            b"Battery power profile screen brightness timeout standby conservation.",
        );

        let mut matches_128 = [RagMatch::default(); 2];
        let mut matches_256 = [RagMatch::default(); 2];

        let c128 = index.search(b"battery power standby", 128, 2, 5, &mut matches_128);
        let c256 = index.search(b"battery power standby", 256, 2, 5, &mut matches_256);

        assert!(c128 > 0);
        assert!(c256 > 0);
        assert_eq!(
            &matches_128[0].source_path[..matches_128[0].source_path_len as usize],
            b"/storage/notes/power.txt"
        );
        assert_eq!(
            &matches_256[0].source_path[..matches_256[0].source_path_len as usize],
            b"/storage/notes/power.txt"
        );
    }

    #[test]
    fn test_format_rag_prompt_exact_citations() {
        let mut matches = [RagMatch::default(); 1];
        matches[0].chunk_id = 0x1234_5678;
        let path = b"/storage/notes/specs.md";
        matches[0].source_path[..path.len()].copy_from_slice(path);
        matches[0].source_path_len = path.len() as u8;
        matches[0].start_offset = 10;
        matches[0].end_offset = 120;
        matches[0].confidence_percent = 91;
        matches[0].is_neural = true;
        let snippet = b"Microkernel verified on QEMU x86_64";
        matches[0].snippet[..snippet.len()].copy_from_slice(snippet);
        matches[0].snippet_len = snippet.len() as u16;

        let mut buf = [0u8; 512];
        let len = format_rag_prompt(b"What target is verified?", &matches, &mut buf).expect("formatted");
        let prompt_str = core::str::from_utf8(&buf[..len]).expect("valid utf8");

        assert!(prompt_str.contains("/storage/notes/specs.md"));
        assert!(prompt_str.contains("offset: 10-120"));
        assert!(prompt_str.contains("confidence: 91%"));
        assert!(prompt_str.contains("mode: neural"));
        assert!(prompt_str.contains("Microkernel verified on QEMU x86_64"));
        assert!(prompt_str.contains("What target is verified?"));
    }

    #[test]
    fn test_format_rag_prompt_empty_citations_truthful() {
        let matches = [];
        let mut buf = [0u8; 512];
        let len = format_rag_prompt(b"Explain relativity", &matches, &mut buf).expect("formatted");
        let prompt_str = core::str::from_utf8(&buf[..len]).expect("valid utf8");

        assert!(prompt_str.contains("[No matching local documents retrieved from storage]"));
        assert!(prompt_str.contains("Explain relativity"));
    }
}
