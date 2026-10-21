//! The content layer's SDK surface: the shared document index and the
//! system-wide intent registry.
//!
//! WHY these calls take no capability slot: a slot is a position in the
//! task's capability table, and the position depends on the order the kernel
//! inserted the capabilities in. The platform authorises by capability type,
//! so the caller's authority is implied by the call itself.
//!
//! WHY the records are text: an app in this system has no heap, so a record
//! it can read without allocating is worth more than a compact record it
//! cannot. The text form is also the form the kernel logs and the form the
//! contract test reads, so there is one spelling of a document.

use crate::{syscall0, syscall2, syscall5, SyscallNumber};

/// The kernel refused the call: the caller holds no such authority.
pub const ERR_PERMISSION_DENIED: i64 = -1;
/// The kernel refused the call: the arguments were not usable.
pub const ERR_INVALID_ARGUMENTS: i64 = -3;
/// The kernel refused the call: nothing handles the intent.
///
/// WHY this is distinct from a permission failure: a permission failure says
/// "you may not ask", and this says "you may ask, and the answer is that
/// there is no handler". An app that gets this must say that no app handles
/// the thing, and must name nothing.
pub const ERR_NO_HANDLER: i64 = -6;

/// The kind of thing a handler declares it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    /// A media type, e.g. `image/png`.
    Mime,
    /// A URI scheme, e.g. `https`.
    Scheme,
    /// A named action, e.g. `org.interos.action.set_wallpaper`.
    Action,
}

impl IntentKind {
    /// The wire value of the kind.
    pub const fn as_raw(self) -> u64 {
        match self {
            IntentKind::Mime => 0,
            IntentKind::Scheme => 1,
            IntentKind::Action => 2,
        }
    }

    /// The wire value of the kind, as it appears in a listing.
    pub const fn as_str(self) -> &'static str {
        match self {
            IntentKind::Mime => "mime",
            IntentKind::Scheme => "scheme",
            IntentKind::Action => "action",
        }
    }

    /// Resolves the wire spelling of a kind back to the kind.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "mime" => Some(IntentKind::Mime),
            "scheme" => Some(IntentKind::Scheme),
            "action" => Some(IntentKind::Action),
            _ => None,
        }
    }
}

/// The action that fills the wallpaper slot of the home surface.
pub const ACTION_SET_WALLPAPER: &str = "org.interos.action.set_wallpaper";
/// The action that fills the wallpaper slot of the lock surface.
pub const ACTION_SET_LOCK_WALLPAPER: &str = "org.interos.action.set_lock_wallpaper";
/// The action that fills the ringtone slot.
pub const ACTION_SET_RINGTONE: &str = "org.interos.action.set_ringtone";

/// A document the platform's index holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentRecord<'a> {
    /// The stable identity of the document.
    pub id: u64,
    /// The absolute path of the document.
    pub path: &'a str,
    /// The media type of the document.
    pub mime: &'a str,
    /// Where the media type answer came from: `bytes`, `extension` or
    /// `unknown`.
    pub provenance: &'a str,
    /// The length of the document in bytes.
    pub length: u64,
    /// The modification time in the platform's tick counter.
    pub mtime: u64,
}

impl<'a> DocumentRecord<'a> {
    /// Parses one index record.
    ///
    /// WHY a record that does not parse is skipped rather than reported: the
    /// index is a platform surface, and a caller that cannot read one line of
    /// it should still be able to read the rest. A caller that needs every
    /// record counts what it got.
    pub fn parse(line: &'a str) -> Option<Self> {
        let mut parts = line.split('|');
        let id = u64::from_str_radix(parts.next()?.trim(), 16).ok()?;
        let path = parts.next()?;
        let mime = parts.next()?;
        let provenance = parts.next()?;
        let length = parts.next()?.trim().parse::<u64>().ok()?;
        let mtime = parts.next()?.trim().parse::<u64>().ok()?;
        Some(DocumentRecord {
            id,
            path,
            mime,
            provenance,
            length,
            mtime,
        })
    }

    /// Whether the media type was read from the document's own bytes.
    pub fn mime_from_bytes(&self) -> bool {
        self.provenance == "bytes"
    }
}

/// Parses the index's answer into a fixed array of records.
///
/// WHY a fixed array: the apps in this system have no heap, so the caller
/// says how many records it can hold and the platform fills what it can.
pub fn parse_index<'a>(buf: &'a [u8], out: &mut [DocumentRecord<'a>]) -> usize {
    let mut count = 0usize;
    for line in buf.split(|byte| *byte == b'\n') {
        if count >= out.len() {
            break;
        }
        let Ok(text) = core::str::from_utf8(line) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if let Some(record) = DocumentRecord::parse(text) {
            out[count] = record;
            count += 1;
        }
    }
    count
}

/// A handler an installed package declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntentHandler<'a> {
    /// What sort of thing the handler takes.
    pub kind: IntentKind,
    /// The thing the handler takes.
    pub value: &'a str,
    /// The package that declared it.
    pub package_id: &'a str,
}

/// Parses the registry's listing into a fixed array of handlers.
pub fn parse_intents<'a>(buf: &'a [u8], out: &mut [IntentHandler<'a>]) -> usize {
    let mut count = 0usize;
    for line in buf.split(|byte| *byte == b'\n') {
        if count >= out.len() {
            break;
        }
        let Ok(text) = core::str::from_utf8(line) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let mut parts = text.split('|');
        let Some(kind) = parts.next().and_then(IntentKind::from_str) else {
            continue;
        };
        let Some(value) = parts.next() else {
            continue;
        };
        let Some(package_id) = parts.next() else {
            continue;
        };
        out[count] = IntentHandler {
            kind,
            value,
            package_id,
        };
        count += 1;
    }
    count
}

/// An intent that was dispatched to the calling package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingIntent<'a> {
    /// What sort of thing the intent was.
    pub kind: IntentKind,
    /// The thing the intent was about.
    pub value: &'a str,
    /// The payload, normally a document path.
    pub payload: &'a str,
    /// The dispatch counter, so a caller can tell a fresh intent from a
    /// stale one.
    pub token: u64,
}

/// Parses the pending intent's answer.
pub fn parse_pending(buf: &[u8]) -> Option<PendingIntent<'_>> {
    let text = core::str::from_utf8(buf).ok()?;
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return None;
    }
    let mut parts = text.split('|');
    let kind = IntentKind::from_str(parts.next()?)?;
    let value = parts.next()?;
    let payload = parts.next()?;
    let token = parts.next().unwrap_or("0").trim().parse::<u64>().unwrap_or(0);
    Some(PendingIntent {
        kind,
        value,
        payload,
        token,
    })
}

/// Queries the shared document index.
///
/// The query kinds are [`QUERY_ALL`], [`QUERY_NAME`], [`QUERY_MIME`] and
/// [`QUERY_ID`].
pub fn content_index_query(query: u64, param: &str, out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall5(
            SyscallNumber::ContentIndexQuery,
            query,
            param.as_ptr() as u64,
            param.len() as u64,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Lists every document the platform's index holds.
pub fn content_index_list(out: &mut [u8]) -> Result<usize, i64> {
    content_index_query(QUERY_ALL, "", out)
}

/// Searches the index by document name.
pub fn content_index_search_name(needle: &str, out: &mut [u8]) -> Result<usize, i64> {
    content_index_query(QUERY_NAME, needle, out)
}

/// Searches the index by media type.
pub fn content_index_search_mime(mime: &str, out: &mut [u8]) -> Result<usize, i64> {
    content_index_query(QUERY_MIME, mime, out)
}

/// Looks one document up by its identity.
pub fn content_index_lookup(id: u64, out: &mut [u8]) -> Result<usize, i64> {
    let mut hex = [0u8; 16];
    id_hex_into(id, &mut hex);
    let param = core::str::from_utf8(&hex).unwrap_or("");
    content_index_query(QUERY_ID, param, out)
}

/// Writes the identity of a document into a fixed-width hex buffer.
///
/// WHY a buffer and not a `String`: the SDK has no heap, and a caller that
/// wants to show an identity wants sixteen characters, not an allocation.
pub fn id_hex_into(id: u64, out: &mut [u8; 16]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for (index, slot) in out.iter_mut().enumerate() {
        let shift = (15 - index) * 4;
        *slot = DIGITS[((id >> shift) & 0xF) as usize];
    }
}

/// Forces a fresh walk of the shared storage area.
pub fn content_index_refresh() -> Result<usize, i64> {
    let r = unsafe { syscall0(SyscallNumber::ContentIndexRefresh) };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Resolves the package that handles a media type, scheme or action.
///
/// Returns [`ERR_NO_HANDLER`] when nothing declares a handler for it. The
/// caller must report that no app handles the thing and must name nothing.
pub fn intent_resolve(kind: IntentKind, value: &str, out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall5(
            SyscallNumber::IntentResolve,
            kind.as_raw(),
            value.as_ptr() as u64,
            value.len() as u64,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Lists every handler the installed packages have declared.
pub fn intent_list(out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::IntentList,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// Dispatches an intent to the package that declared a handler for it.
///
/// The payload is normally a document path. Returns the identifier of the
/// task that was launched, or [`ERR_NO_HANDLER`] when nothing declares a
/// handler for the intent.
pub fn intent_dispatch(kind: IntentKind, value: &str, payload: &str) -> Result<u64, i64> {
    let r = unsafe {
        syscall5(
            SyscallNumber::IntentDispatch,
            kind.as_raw(),
            value.as_ptr() as u64,
            value.len() as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as u64)
    } else {
        Err(r)
    }
}

/// The intent dispatched to the calling package, if there is one.
pub fn intent_pending(out: &mut [u8]) -> Result<usize, i64> {
    let r = unsafe {
        syscall2(
            SyscallNumber::IntentPending,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if r >= 0 {
        Ok(r as usize)
    } else {
        Err(r)
    }
}

/// The query that lists every document.
pub const QUERY_ALL: u64 = 0;
/// The query that searches documents by name.
pub const QUERY_NAME: u64 = 1;
/// The query that searches documents by media type.
pub const QUERY_MIME: u64 = 2;
/// The query that looks a document up by its identity.
pub const QUERY_ID: u64 = 3;

/// Finds the document at a path in the platform's index.
///
/// WHY this and not a directory listing: a path is not enough to say what a
/// document is, and the index is where the platform holds the media type and
/// the identity of a document.
pub fn content_index_find_path<'a>(
    path: &str,
    raw: &'a mut [u8],
) -> Option<DocumentRecord<'a>> {
    let n = content_index_query(QUERY_ALL, "", &mut *raw).ok()?;
    let text: &'a str = core::str::from_utf8(&raw[..n]).ok()?;
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let record = DocumentRecord::parse(line)?;
        if record.path == path {
            return Some(record);
        }
    }
    None
}

/// The identity of a document, derived from its path.
///
/// WHY the SDK computes it: an app that holds a document's path needs the
/// document's identity to ask about that document, and the derivation is a
/// pure function of the path so both sides agree without a round trip.
pub fn document_id(path: &str) -> u64 {
    const TAG: u64 = 0x444F_4300_0000_0000;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    TAG | (hash & 0x0000_FFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_record_round_trips() {
        let line = "444f4300000000ab|/storage/photos/shot.ppm|image/x-portable-pixmap|bytes|2048|77";
        let record = DocumentRecord::parse(line).expect("the record parses");
        assert_eq!(record.path, "/storage/photos/shot.ppm");
        assert_eq!(record.mime, "image/x-portable-pixmap");
        assert_eq!(record.provenance, "bytes");
        assert_eq!(record.length, 2048);
        assert_eq!(record.mtime, 77);
        assert!(record.mime_from_bytes());
    }

    #[test]
    fn a_record_that_does_not_parse_is_not_a_record() {
        assert!(DocumentRecord::parse("not a record").is_none());
        assert!(DocumentRecord::parse("").is_none());
    }

    #[test]
    fn the_index_answer_is_parsed_into_records() {
        let answer = b"1111|/storage/a.ppm|image/x-portable-pixmap|bytes|10|1\n2222|/storage/b.ppm|image/x-portable-pixmap|bytes|20|2\n";
        let mut records = [DocumentRecord {
            id: 0,
            path: "",
            mime: "",
            provenance: "",
            length: 0,
            mtime: 0,
        }; 4];
        let count = parse_index(answer, &mut records);
        assert_eq!(count, 2);
        assert_eq!(records[0].path, "/storage/a.ppm");
        assert_eq!(records[1].length, 20);
    }

    #[test]
    fn the_registry_answer_is_parsed_into_handlers() {
        let answer = b"mime|image/png|org.interos.gallery\naction|org.interos.action.set_wallpaper|org.interos.settings\n";
        let mut handlers = [IntentHandler {
            kind: IntentKind::Mime,
            value: "",
            package_id: "",
        }; 4];
        let count = parse_intents(answer, &mut handlers);
        assert_eq!(count, 2);
        assert_eq!(handlers[0].kind, IntentKind::Mime);
        assert_eq!(handlers[0].package_id, "org.interos.gallery");
        assert_eq!(handlers[1].kind, IntentKind::Action);
    }

    #[test]
    fn a_pending_intent_round_trips() {
        let answer = b"action|org.interos.action.set_wallpaper|/storage/photos/shot.ppm|7\n";
        let pending = parse_pending(answer).expect("the intent parses");
        assert_eq!(pending.kind, IntentKind::Action);
        assert_eq!(pending.value, "org.interos.action.set_wallpaper");
        assert_eq!(pending.payload, "/storage/photos/shot.ppm");
        assert_eq!(pending.token, 7);
    }

    #[test]
    fn an_empty_pending_answer_is_no_intent() {
        assert!(parse_pending(b"").is_none());
        assert!(parse_pending(b"\n").is_none());
    }

    #[test]
    fn the_identity_of_a_path_is_stable() {
        let a = document_id("/storage/photos/a.ppm");
        assert_eq!(a, document_id("/storage/photos/a.ppm"));
        assert_ne!(a, document_id("/storage/photos/b.ppm"));
    }

    #[test]
    fn the_identity_is_written_as_a_fixed_width_hex() {
        let mut hex = [0u8; 16];
        id_hex_into(0x444f_4300_0000_00ab, &mut hex);
        assert_eq!(hex.len(), 16);
        assert_eq!(&hex[12..], b"00ab");
    }
}
