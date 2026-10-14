#![no_std]

pub use interos_std as std;

pub const IMAGE_SUFFIXES: [&[u8]; 6] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp",
];

/// Whether an entry name carries a recognised image extension (case-insensitive).
pub fn has_image_suffix(name: &[u8]) -> bool {
    IMAGE_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Counts total entries and image files in a VFS `fs_list` payload formatted
/// as newline-separated `name|size|type` lines.
pub fn count_listing(payload: &[u8]) -> (usize, usize) {
    let mut entries = 0usize;
    let mut images = 0usize;
    for line in payload.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        entries += 1;
        let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
        if has_image_suffix(&line[..end]) {
            images += 1;
        }
    }
    (entries, images)
}

/// Appends `bytes` to `buf` at `n` and returns the new length, truncating
/// rather than overrunning.
pub fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of `value` to `buf` at `n`.
pub fn append_u64(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
    if value == 0 {
        return append(buf, n, b"0");
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 && len < 20 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

/// One image the gallery can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Wallpaper(usize),
    Photo(usize),
}

/// Which set the filter row selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Wallpapers,
    Photos,
}

impl Filter {
    pub fn from_index(index: usize) -> Filter {
        match index {
            1 => Filter::Wallpapers,
            2 => Filter::Photos,
            _ => Filter::All,
        }
    }

    pub fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    pub fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }
}

/// Resolves the media item at a flat grid index.
pub fn media_at(index: usize, wallpaper_count: usize, photo_count: usize) -> Option<Media> {
    if index < wallpaper_count {
        return Some(Media::Wallpaper(index));
    }
    let photo = index - wallpaper_count;
    if photo < photo_count {
        return Some(Media::Photo(photo));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_suffix_detection() {
        assert!(has_image_suffix(b"photo.jpg"));
        assert!(has_image_suffix(b"IMAGE.PNG"));
        assert!(has_image_suffix(b"graphic.bmp"));
        assert!(has_image_suffix(b"anim.gif"));
        assert!(has_image_suffix(b"shot.webp"));
        assert!(has_image_suffix(b"picture.jpeg"));

        assert!(!has_image_suffix(b"document.txt"));
        assert!(!has_image_suffix(b"binary.exe"));
        assert!(!has_image_suffix(b"manifest.json"));
        assert!(!has_image_suffix(b".png")); // suffix without base name
        assert!(!has_image_suffix(b""));
    }

    #[test]
    fn test_count_listing() {
        let payload = b"wallpaper.jpg|10240|file\nreadme.txt|512|file\npic.PNG|2048|file\nempty_dir|0|dir\n";
        let (entries, images) = count_listing(payload);
        assert_eq!(entries, 4);
        assert_eq!(images, 2);
    }

    #[test]
    fn test_append_u64_formatting() {
        let mut buf = [0u8; 32];
        let n = append_u64(&mut buf, 0, 0);
        assert_eq!(&buf[..n], b"0");

        let n2 = append_u64(&mut buf, 0, 1440);
        assert_eq!(&buf[..n2], b"1440");

        let n3 = append_u64(&mut buf, 0, 9876543210);
        assert_eq!(&buf[..n3], b"9876543210");
    }

    #[test]
    fn test_filter_and_media_at() {
        let f_all = Filter::from_index(0);
        assert!(f_all.shows_wallpapers());
        assert!(f_all.shows_ledger());

        let f_wall = Filter::from_index(1);
        assert!(f_wall.shows_wallpapers());
        assert!(!f_wall.shows_ledger());

        let f_photo = Filter::from_index(2);
        assert!(!f_photo.shows_wallpapers());
        assert!(f_photo.shows_ledger());

        assert_eq!(media_at(0, 3, 2), Some(Media::Wallpaper(0)));
        assert_eq!(media_at(2, 3, 2), Some(Media::Wallpaper(2)));
        assert_eq!(media_at(3, 3, 2), Some(Media::Photo(0)));
        assert_eq!(media_at(4, 3, 2), Some(Media::Photo(1)));
        assert_eq!(media_at(5, 3, 2), None);
    }
}
