// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The platform's content layer: a shared document index and a system-wide
//! intent registry.
//!
//! WHY this is a kernel subsystem and not a service that happens to be
//! convenient: both halves of it answer questions about the whole device --
//! "what documents does this device hold" and "which program handles this
//! one" -- and a question with a device-wide answer cannot be answered by one
//! of the programs it is about. Both halves are reachable only through a
//! capability, so "who may see the index" is answered by the capability table
//! and not by a constant in this file.
//!
//! WHAT the two halves share: the index is where the documents are described,
//! and the registry is where the handlers are described. Together they are
//! what lets one app act on a document another app produced.

pub mod index;
pub mod intent;
pub mod mime;

use alloc::string::ToString;

pub use index::{document_id, DocumentIndex, DocumentRecord, CONTENT_INDEX, MIME_SNIFF_BYTES};
pub use intent::{
    IntentDeclaration, IntentHandler, IntentKind, IntentRegistry, PendingIntent, INTENT_REGISTRY,
};
pub use mime::{MimeProvenance, MimeType};

/// The root of the shared storage area the index describes.
///
/// WHY the shared area and not the whole filesystem: `/storage` is the one
/// area every app may see, so it is the one area a shared index can describe
/// without telling one app about another's private files.
pub const SHARED_STORAGE_ROOT: &str = "/storage";

/// The document the boot check writes into the shared area.
const SELF_TEST_PATH: &str = "/storage/photos/interos_content_self_test.ppm";

/// Seeds the intent registry and checks the content layer against the device.
///
/// WHY the check runs at boot: the registry is the answer to "which program
/// opens this file?", and an answer that is only correct in a unit test is
/// not an answer. The check resolves a media type a package declared, refuses
/// one that no package declared, writes a document into the shared area and
/// confirms the index describes it from the document's own bytes — and then
/// removes the document, because a device must not be left holding a file
/// that only exists to prove a point.
pub fn init() {
    crate::package::seed_intent_registry();

    let declared = {
        let registry = INTENT_REGISTRY.lock();
        registry.resolve(IntentKind::Mime, "image/png").map(|id| id.to_string())
    };
    match declared.as_deref() {
        Some("org.interos.gallery") => {
            crate::serial_println!(
                "[TEST] Content layer: the registry resolved a declared media type to the package that declared it."
            );
        }
        other => {
            crate::serial_println!(
                "[TEST] Content layer: the registry did not resolve a declared media type ({:?}); the handler table is wrong.",
                other
            );
        }
    }

    {
        let registry = INTENT_REGISTRY.lock();
        if registry.resolve(IntentKind::Mime, "application/x-interos-unhandled").is_some() {
            crate::serial_println!(
                "[TEST] Content layer: a media type no package declared resolved to a handler; an unhandled media type must be refused."
            );
        } else {
            crate::serial_println!(
                "[TEST] Content layer: a media type no package declared is refused, and no handler is named."
            );
        }
    }

    // The index half: a real document in the shared area, described from its
    // own bytes.
    let ppm = b"P6\n2 2\n255\n\xff\x00\x00\x00\xff\x00\x00\x00\xff\x00\xff\x00\xff";
    {
        let mut vfs = crate::fs::VFS.lock();
        vfs.create_file(SELF_TEST_PATH, ppm, false);
    }
    let record = {
        let vfs = crate::fs::VFS.lock();
        let generation = vfs.storage_generation();
        let files = vfs.walk_files(SHARED_STORAGE_ROOT, MIME_SNIFF_BYTES);
        let mut index = CONTENT_INDEX.lock();
        index.replace(&files, generation);
        index
            .records()
            .iter()
            .find(|record| record.path == SELF_TEST_PATH)
            .cloned()
    };
    {
        let mut vfs = crate::fs::VFS.lock();
        let _ = vfs.unlink(SELF_TEST_PATH);
    }
    match record {
        Some(record)
            if record.mime == "image/x-portable-pixmap" && record.provenance == MimeProvenance::Bytes =>
        {
            crate::serial_println!(
                "[TEST] Content layer: the index described a document in the shared area, and read its media type from its own bytes."
            );
        }
        Some(record) => {
            crate::serial_println!(
                "[TEST] Content layer: the index described the document as {} ({:?}); it should have been read from the bytes.",
                record.mime,
                record.provenance
            );
        }
        None => {
            crate::serial_println!(
                "[TEST] Content layer: the index did not describe the document written into the shared area."
            );
        }
    }
}
