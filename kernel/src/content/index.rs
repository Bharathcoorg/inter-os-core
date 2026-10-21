//! The shared document index.
//!
//! WHAT this is: one walk of the shared storage area, held by the kernel, with
//! a record per file carrying the path, the byte length, the media type, the
//! modification time and a stable identity. Every app that is allowed to see
//! the shared area sees the same records, so "what files does this device
//! have" has one answer instead of one per app.
//!
//! WHY the index is not a cache of the filesystem: the filesystem is the
//! truth, and this is a description of it that is rebuilt whenever the
//! filesystem changes. A stale record would be worse than no record, so the
//! index carries the storage generation it was built from and is rebuilt when
//! that generation moves.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::content::mime::{self, MimeProvenance};

/// The namespace tag of a document identity.
///
/// WHY a tag: the identity is a 64-bit value that travels next to other
/// 64-bit values, and an identity that could be confused with a capability
/// id, a task id or a token would be a hazard rather than a convenience.
const DOCUMENT_ID_TAG: u64 = 0x444F_4300_0000_0000;

/// The number of leading bytes of a file the index reads to classify it.
pub const MIME_SNIFF_BYTES: usize = 16;

/// The stable identity of a document.
///
/// WHY the path and not the contents: the identity has to survive an edit of
/// the file, or every reference to it would break the moment a document is
/// touched. The path is the thing a caller has, so it is the thing the id is
/// derived from.
pub fn document_id(path: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    DOCUMENT_ID_TAG | (hash & 0x0000_FFFF_FFFF_FFFF)
}

/// One document the platform holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRecord {
    /// The stable identity of the document.
    pub id: u64,
    /// The absolute path of the document.
    pub path: String,
    /// The length of the document in bytes.
    pub length: u64,
    /// The media type of the document.
    pub mime: &'static str,
    /// Where the media type answer came from.
    pub provenance: MimeProvenance,
    /// The modification time, in the platform's tick counter.
    pub mtime: u64,
}

impl DocumentRecord {
    /// Builds a record from a walked file.
    pub fn from_walked(path: &str, length: u64, mtime: u64, head: &[u8]) -> Self {
        let name = mime::file_name(path);
        let detected = mime::classify(name, head);
        Self {
            id: document_id(path),
            path: path.to_string(),
            length,
            mime: detected.mime,
            provenance: detected.provenance,
            mtime,
        }
    }

    /// The file name of the document, without its directory.
    pub fn name(&self) -> &str {
        mime::file_name(&self.path)
    }

    /// Whether the document's name contains `needle`, case-insensitively.
    ///
    /// WHY the name and not the whole path: a caller searching for "report"
    /// wants the documents called report, not every document under a directory
    /// that happens to be called reports.
    pub fn name_contains(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let name = self.name().as_bytes();
        let needle = needle.as_bytes();
        if needle.len() > name.len() {
            return false;
        }
        name.windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    }
}

/// The device's shared document index.
#[derive(Debug, Clone)]
pub struct DocumentIndex {
    records: Vec<DocumentRecord>,
    /// The storage generation the records were built from.
    generation: u64,
    /// Whether a walk has happened at all.
    scanned: bool,
}

impl DocumentIndex {
    /// An index that has not walked anything yet.
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
            generation: 0,
            scanned: false,
        }
    }

    /// Every record the index holds.
    pub fn records(&self) -> &[DocumentRecord] {
        &self.records
    }

    /// Whether the index reflects the given storage generation.
    pub fn is_current(&self, generation: u64) -> bool {
        self.scanned && self.generation == generation
    }

    /// Replaces the index's contents with a fresh walk.
    pub fn replace(&mut self, files: &[(String, u64, u64, Vec<u8>)], generation: u64) {
        let mut records = Vec::with_capacity(files.len());
        for (path, length, mtime, head) in files {
            records.push(DocumentRecord::from_walked(path, *length, *mtime, head));
        }
        records.sort_by(|a, b| a.path.cmp(&b.path));
        self.records = records;
        self.generation = generation;
        self.scanned = true;
    }

    /// The record with a given identity.
    pub fn find_by_id(&self, id: u64) -> Option<&DocumentRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    /// Every record whose media type is exactly `mime`.
    pub fn search_by_mime(&self, mime: &str) -> Vec<&DocumentRecord> {
        self.records
            .iter()
            .filter(|record| record.mime == mime)
            .collect()
    }

    /// Every record whose name contains `needle`.
    pub fn search_by_name(&self, needle: &str) -> Vec<&DocumentRecord> {
        self.records
            .iter()
            .filter(|record| record.name_contains(needle))
            .collect()
    }
}

/// The device's shared document index.
pub static CONTENT_INDEX: spin::Mutex<DocumentIndex> = spin::Mutex::new(DocumentIndex::new());

#[cfg(test)]
mod tests {
    use super::*;

    fn walked(path: &str, head: &[u8]) -> (String, u64, u64, Vec<u8>) {
        (path.to_string(), 128, 42, head.to_vec())
    }

    #[test]
    fn a_record_carries_the_path_length_mtime_and_media_type() {
        let record = DocumentRecord::from_walked(
            "/storage/photos/shot.ppm",
            2048,
            77,
            b"P6\n36 72\n255\n",
        );
        assert_eq!(record.path, "/storage/photos/shot.ppm");
        assert_eq!(record.length, 2048);
        assert_eq!(record.mtime, 77);
        assert_eq!(record.mime, "image/x-portable-pixmap");
        assert_eq!(record.provenance, MimeProvenance::Bytes);
    }

    #[test]
    fn the_identity_of_a_path_is_stable_and_distinct() {
        let a = document_id("/storage/photos/a.ppm");
        let b = document_id("/storage/photos/b.ppm");
        assert_eq!(a, document_id("/storage/photos/a.ppm"));
        assert_ne!(a, b);
        assert_ne!(a & 0x0000_FFFF_FFFF_FFFF, b & 0x0000_FFFF_FFFF_FFFF);
    }

    #[test]
    fn the_index_answers_a_lookup_by_identity() {
        let mut index = DocumentIndex::new();
        index.replace(
            &[
                walked("/storage/photos/a.ppm", b"P6\n"),
                walked("/storage/photos/b.ppm", b"P6\n"),
            ],
            1,
        );
        let id = document_id("/storage/photos/b.ppm");
        let found = index.find_by_id(id).expect("the record is present");
        assert_eq!(found.path, "/storage/photos/b.ppm");
        assert!(index.find_by_id(document_id("/storage/nope.ppm")).is_none());
    }

    #[test]
    fn the_index_searches_by_name_and_by_media_type() {
        let mut index = DocumentIndex::new();
        index.replace(
            &[
                walked("/storage/photos/holiday.ppm", b"P6\n"),
                walked("/storage/photos/portrait.ppm", b"P6\n"),
                walked("/storage/downloads/page.html", b"<html>"),
            ],
            1,
        );
        let by_name = index.search_by_name("port");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].path, "/storage/photos/portrait.ppm");
        let by_mime = index.search_by_mime("image/x-portable-pixmap");
        assert_eq!(by_mime.len(), 2);
        let by_html = index.search_by_mime("text/html");
        assert_eq!(by_html.len(), 1);
    }

    #[test]
    fn the_index_reports_when_it_is_out_of_date() {
        let mut index = DocumentIndex::new();
        assert!(!index.is_current(0));
        index.replace(&[], 7);
        assert!(index.is_current(7));
        assert!(!index.is_current(8));
    }
}
