#![no_std]
//! InterOS File Manager VFS parser and data models.

extern crate interos_std;

pub const MAX_PARSED_ENTRIES: usize = 8;
pub const MAX_ENTRY_NAME_LEN: usize = 32;

/// A bounded, zero-heap directory entry parsed from kernel VFS `fs_list`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RuntimeFileEntry {
    pub name: [u8; MAX_ENTRY_NAME_LEN],
    pub name_len: usize,
    pub size: u64,
    pub size_label: [u8; 16],
    pub size_label_len: usize,
    pub is_dir: bool,
}

impl RuntimeFileEntry {
    pub const EMPTY: Self = Self {
        name: [0u8; MAX_ENTRY_NAME_LEN],
        name_len: 0,
        size: 0,
        size_label: [0u8; 16],
        size_label_len: 0,
        is_dir: false,
    };
}

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
pub fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Parses newline-delimited VFS entries in `<name>|<size>|<dir|dev|file>` format.
pub fn parse_vfs_entries(data: &[u8], entries: &mut [RuntimeFileEntry; MAX_PARSED_ENTRIES]) -> usize {
    let mut entry_count = 0;
    let mut start = 0;
    while start < data.len() && entry_count < MAX_PARSED_ENTRIES {
        let end = data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p)
            .unwrap_or(data.len());
        let line = &data[start..end];
        if !line.is_empty() {
            let mut parts = line.split(|&b| b == b'|');
            if let (Some(name_bytes), Some(size_bytes), Some(type_bytes)) =
                (parts.next(), parts.next(), parts.next())
            {
                let mut entry = RuntimeFileEntry::EMPTY;
                let n = name_bytes.len().min(MAX_ENTRY_NAME_LEN);
                entry.name[..n].copy_from_slice(&name_bytes[..n]);
                entry.name_len = n;
                let mut size: u64 = 0;
                for &b in size_bytes {
                    if b.is_ascii_digit() {
                        size = size.saturating_mul(10).saturating_add((b - b'0') as u64);
                    }
                }
                entry.size = size;
                entry.is_dir = type_bytes == b"dir";
                if entry.is_dir {
                    entry.size_label[..3].copy_from_slice(b"DIR");
                    entry.size_label_len = 3;
                } else {
                    let sn = fmt_u64(entry.size, &mut entry.size_label);
                    if sn + 2 <= 16 {
                        entry.size_label[sn..sn + 2].copy_from_slice(b" B");
                        entry.size_label_len = sn + 2;
                    } else {
                        entry.size_label_len = sn;
                    }
                }
                entries[entry_count] = entry;
                entry_count += 1;
            }
        }
        start = end.saturating_add(1);
    }
    entry_count
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileMime {
    Directory,
    Image,
    Video,
    Audio,
    Document,
    Other,
}

pub const IMAGE_SUFFIXES: [&[u8]; 9] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp", b".qoi", b".ppm", b".raw",
];

pub const VIDEO_SUFFIXES: [&[u8]; 5] = [
    b".mp4", b".webm", b".mkv", b".avi", b".mov",
];

pub const AUDIO_SUFFIXES: [&[u8]; 6] = [
    b".mp3", b".wav", b".ogg", b".flac", b".aac", b".m4a",
];

pub const DOC_SUFFIXES: [&[u8]; 7] = [
    b".txt", b".pdf", b".md", b".doc", b".json", b".log", b".csv",
];

pub fn has_suffix(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

pub fn detect_mime(name: &[u8], is_dir: bool) -> FileMime {
    if is_dir {
        return FileMime::Directory;
    }
    if IMAGE_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Image;
    }
    if VIDEO_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Video;
    }
    if AUDIO_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Audio;
    }
    if DOC_SUFFIXES.iter().any(|s| has_suffix(name, s)) {
        return FileMime::Document;
    }
    FileMime::Other
}

pub fn default_app_for_mime(mime: FileMime) -> &'static str {
    match mime {
        FileMime::Image | FileMime::Video => "org.interos.gallery",
        FileMime::Audio => "org.interos.music",
        FileMime::Document => "org.interos.terminal",
        _ => "",
    }
}

pub fn get_app_for_mime(mime: FileMime, out: &mut [u8]) -> usize {
    let key = match mime {
        FileMime::Image => "mime.image",
        FileMime::Video => "mime.video",
        FileMime::Audio => "mime.audio",
        FileMime::Document => "mime.document",
        _ => return 0,
    };
    if let Ok(n) = interos_std::settings_get(key, out) {
        if n > 0 {
            return n;
        }
    }
    let def = default_app_for_mime(mime).as_bytes();
    let n = def.len().min(out.len());
    out[..n].copy_from_slice(&def[..n]);
    n
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vfs_entries() {
        let raw = b"media|0|dir\nlogs|4096|dir\nconfig.json|512|file\nsystem.bin|1048576|file\n";
        let mut entries = [RuntimeFileEntry::EMPTY; MAX_PARSED_ENTRIES];
        let count = parse_vfs_entries(raw, &mut entries);
        assert_eq!(count, 4);
        assert_eq!(&entries[0].name[..entries[0].name_len], b"media");
        assert!(entries[0].is_dir);
        assert_eq!(&entries[0].size_label[..entries[0].size_label_len], b"DIR");
        assert_eq!(&entries[1].name[..entries[1].name_len], b"logs");
        assert!(entries[1].is_dir);
        assert_eq!(&entries[2].name[..entries[2].name_len], b"config.json");
        assert!(!entries[2].is_dir);
        assert_eq!(entries[2].size, 512);
        assert_eq!(&entries[2].size_label[..entries[2].size_label_len], b"512 B");
    }

    #[test]
    fn test_detect_mime() {
        assert_eq!(detect_mime(b"folder", true), FileMime::Directory);
        assert_eq!(detect_mime(b"photo.jpg", false), FileMime::Image);
        assert_eq!(detect_mime(b"screenshot.ppm", false), FileMime::Image);
        assert_eq!(detect_mime(b"clip.mp4", false), FileMime::Video);
        assert_eq!(detect_mime(b"song.mp3", false), FileMime::Audio);
        assert_eq!(detect_mime(b"notes.txt", false), FileMime::Document);
        assert_eq!(detect_mime(b"archive.tar", false), FileMime::Other);
    }

    #[test]
    fn test_default_app_for_mime() {
        assert_eq!(default_app_for_mime(FileMime::Image), "org.interos.gallery");
        assert_eq!(default_app_for_mime(FileMime::Video), "org.interos.gallery");
        assert_eq!(default_app_for_mime(FileMime::Audio), "org.interos.music");
        assert_eq!(default_app_for_mime(FileMime::Document), "org.interos.terminal");
    }
}
