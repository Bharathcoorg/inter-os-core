//! Bounded inline-image policy for the native reader.
//!
//! This module answers every question about an image that can be answered
//! without a network or a decoder: how large a body is acceptable, what
//! the bytes claim to be, what dimensions the header declares, whether
//! that is inside the decoder's budget, and what an honest placeholder
//! says when the answer is no.
//!
//! It also carries the reader-document marker that records an `<img>`
//! element and the same-origin URL resolution the browser uses before it
//! will fetch an image.
//!
//! The policy is deliberately narrow:
//! - bodies above [`MAX_IMAGE_BODY_BYTES`] are refused, never truncated
//!   into a half-image;
//! - only PNG, JPEG, GIF and lossless WebP are accepted, because those
//!   are the decoders the platform actually has (lossy WebP is refused
//!   by `std::image` and is named as unsupported here);
//! - a declared width or height above [`MAX_IMAGE_DIMENSION`] is refused
//!   before a byte of RGBA is allocated;
//! - images from another origin are not fetched at all.

/// The largest image body the browser will accept.
///
/// WHY this cap: an inline picture is decoded into a static arena and
/// drawn in a scrolling column, so a body larger than a hundred
/// kilobytes is already far beyond what a phone reader can show well.
/// The response is refused rather than cut, because half a PNG is not a
/// PNG.
pub const MAX_IMAGE_BODY_BYTES: usize = 96 * 1024;

/// The largest width or height an inline image may declare.
pub const MAX_IMAGE_DIMENSION: u32 = 512;

/// The most pixels one inline image may declare.
pub const MAX_IMAGE_PIXELS: usize = (MAX_IMAGE_DIMENSION as usize) * (MAX_IMAGE_DIMENSION as usize);

/// How many inline images a page may place.
pub const MAX_INLINE_IMAGES: usize = 4;

/// The decoded-pixel arena, in bytes.
///
/// WHY a shared arena: four images at the per-image cap would be four
/// megabytes. A single arena bounds the total a page can make the
/// browser hold, and an image that does not fit is reported as refused
/// rather than silently dropped.
pub const IMAGE_ARENA_BYTES: usize = 2 * 1024 * 1024;

/// The widest an inline image is displayed, in logical pixels.
pub const MAX_DISPLAY_WIDTH: usize = 672;

/// The tallest an inline image is displayed, in logical pixels.
pub const MAX_DISPLAY_HEIGHT: usize = 720;

/// The height reserved for an image that cannot be shown.
pub const PLACEHOLDER_HEIGHT: usize = 64;

/// The image formats the platform decodes for the reader.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageType {
    Png,
    Jpeg,
    Gif,
    WebP,
    Unsupported,
}

/// Why an image was not shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageRefusal {
    /// The `<img>` element named no usable source.
    NoSource,
    /// The source URL is longer than the marker can carry. The browser
    /// refuses it rather than fetch a cut address.
    SourceTooLong,
    /// The URL uses a scheme this browser does not fetch for images.
    UnsupportedSource,
    /// The body (or the declared length) exceeded the cap.
    TooLarge { bytes: usize, limit: usize },
    /// The bytes are not one of the four supported formats.
    UnsupportedType,
    /// The header declared more pixels than the reader allows.
    DimensionsTooLarge { width: u32, height: u32, limit: u32 },
    /// The header could not be read, or names a variant the decoder
    /// does not implement (progressive JPEG, lossy WebP, 16-bit PNG).
    BadHeader,
    /// The decoder refused the body after it was admitted.
    DecodeFailed,
    /// The image named another origin.
    CrossOrigin,
    /// The page is a local document, so no network fetch is made.
    LocalDocument,
    /// The decoder or the page's arena is full.
    MemoryBudget,
    /// The response did not arrive, or was not a successful one.
    FetchFailed { code: i32 },
    /// More images than the page is allowed to place.
    LimitReached,
}

impl ImageRefusal {
    /// The one-line explanation the placeholder card draws.
    pub fn placeholder(self) -> &'static [u8] {
        match self {
            ImageRefusal::NoSource => b"Image not shown: no source",
            ImageRefusal::SourceTooLong => b"Image not shown: source address too long",
            ImageRefusal::UnsupportedSource => b"Image not shown: unsupported source",
            ImageRefusal::TooLarge { .. } => b"Image not shown: too large",
            ImageRefusal::UnsupportedType => b"Image not shown: unsupported type",
            ImageRefusal::DimensionsTooLarge { .. } => b"Image not shown: dimensions too large",
            ImageRefusal::BadHeader => b"Image not shown: unsupported format variant",
            ImageRefusal::DecodeFailed => b"Image not shown: decode failed",
            ImageRefusal::CrossOrigin => b"Image not shown: different origin",
            ImageRefusal::LocalDocument => b"Image not shown: local document",
            ImageRefusal::MemoryBudget => b"Image not shown: memory budget",
            ImageRefusal::FetchFailed { .. } => b"Image not shown: fetch failed",
            ImageRefusal::LimitReached => b"Image not shown: too many images",
        }
    }
}

/// The result of admitting an image body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageAdmission {
    Accepted {
        kind: ImageType,
        width: u32,
        height: u32,
    },
    Refused(ImageRefusal),
}

/// The state of one placed image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageStatus {
    Decoded,
    Refused(ImageRefusal),
}

impl ImageStatus {
    pub fn is_decoded(self) -> bool {
        self == ImageStatus::Decoded
    }
}

/// Identifies a format from the body's own magic bytes.
///
/// WHY the body and not the Content-Type: a mislabelled response must
/// not make the decoder run the wrong parser. The magic is what the file
/// actually is.
pub fn sniff_image_type(body: &[u8]) -> ImageType {
    if body.len() >= 8 && body[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return ImageType::Png;
    }
    if body.len() >= 3 && body[0] == 0xFF && body[1] == 0xD8 && body[2] == 0xFF {
        return ImageType::Jpeg;
    }
    if body.len() >= 6 && (&body[..6] == b"GIF87a" || &body[..6] == b"GIF89a") {
        return ImageType::Gif;
    }
    if body.len() >= 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WEBP" {
        return ImageType::WebP;
    }
    ImageType::Unsupported
}

/// Whether a declared size fits the decoder's pixel budget.
pub fn check_dimensions(width: u32, height: u32) -> Result<(u32, u32), ImageRefusal> {
    let pixels = (width as usize).checked_mul(height as usize).unwrap_or(usize::MAX);
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || pixels > MAX_IMAGE_PIXELS
    {
        return Err(ImageRefusal::DimensionsTooLarge {
            width,
            height,
            limit: MAX_IMAGE_DIMENSION,
        });
    }
    Ok((width, height))
}

fn probe_dimensions(kind: ImageType, body: &[u8]) -> Result<(u32, u32), ImageRefusal> {
    use crate::std::image;
    match kind {
        ImageType::Png => image::decode_png_header(body).map_err(|_| ImageRefusal::BadHeader),
        ImageType::Jpeg => image::decode_jpeg_header(body).map_err(|_| ImageRefusal::BadHeader),
        ImageType::Gif => image::decode_gif_header(body).map_err(|_| ImageRefusal::BadHeader),
        ImageType::WebP => {
            // The platform decoder handles lossless (VP8L) WebP only. A
            // lossy `VP8 ` chunk is named as an unsupported type rather
            // than reported as a corrupt file.
            if body.len() < 16 || &body[12..16] != b"VP8L" {
                return Err(ImageRefusal::UnsupportedType);
            }
            image::decode_webp_header(body).map_err(|_| ImageRefusal::BadHeader)
        }
        ImageType::Unsupported => Err(ImageRefusal::UnsupportedType),
    }
}

/// Admits or refuses an image body before any decode is attempted.
pub fn admit_image(body: &[u8]) -> ImageAdmission {
    if body.len() > MAX_IMAGE_BODY_BYTES {
        return ImageAdmission::Refused(ImageRefusal::TooLarge {
            bytes: body.len(),
            limit: MAX_IMAGE_BODY_BYTES,
        });
    }
    let kind = sniff_image_type(body);
    if kind == ImageType::Unsupported {
        return ImageAdmission::Refused(ImageRefusal::UnsupportedType);
    }
    match probe_dimensions(kind, body) {
        Ok((w, h)) => match check_dimensions(w, h) {
            Ok((width, height)) => ImageAdmission::Accepted {
                kind,
                width,
                height,
            },
            Err(refusal) => ImageAdmission::Refused(refusal),
        },
        Err(refusal) => ImageAdmission::Refused(refusal),
    }
}

/// The RGBA bytes a decoded image of this size requires.
pub fn required_rgba_bytes(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

/// The size an image is displayed at.
///
/// A declared width or height is honoured; the other side follows the
/// image's aspect ratio. Without declarations the intrinsic size is
/// used, capped to the reader's column and to a bounded height. The
/// renderer samples the decoded bitmap nearest-neighbour, so this is a
/// real scale, not a claim of resampling quality.
pub fn fit_display(
    intrinsic_w: u32,
    intrinsic_h: u32,
    declared_w: Option<u16>,
    declared_h: Option<u16>,
) -> (usize, usize) {
    let iw = intrinsic_w.max(1) as usize;
    let ih = intrinsic_h.max(1) as usize;
    let mut width;
    let mut height;
    match (declared_w, declared_h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            width = w as usize;
            height = h as usize;
        }
        (Some(w), _) if w > 0 => {
            width = w as usize;
            height = (ih.saturating_mul(width) / iw).max(1);
        }
        (_, Some(h)) if h > 0 => {
            height = h as usize;
            width = (iw.saturating_mul(height) / ih).max(1);
        }
        _ => {
            width = iw;
            height = ih;
        }
    }
    if width > MAX_DISPLAY_WIDTH {
        let h = (height.saturating_mul(MAX_DISPLAY_WIDTH) / width).max(1);
        width = MAX_DISPLAY_WIDTH;
        height = h;
    }
    if height > MAX_DISPLAY_HEIGHT {
        let w = (width.saturating_mul(MAX_DISPLAY_HEIGHT) / height).max(1);
        height = MAX_DISPLAY_HEIGHT;
        width = w;
    }
    (width.max(1), height.max(1))
}

/// Why a source URL could not be resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolveError {
    NoSource,
    CrossOrigin,
    UnsupportedScheme,
    InvalidBase,
    TooLong,
}

fn eq_ignore_ascii(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn authority_end(url: &[u8]) -> Option<usize> {
    let start = url.windows(3).position(|w| w == b"://")? + 3;
    let mut i = start;
    while i < url.len() && url[i] != b'/' && url[i] != b'?' && url[i] != b'#' {
        i += 1;
    }
    Some(i)
}

fn split_fragment(src: &[u8]) -> &[u8] {
    match src.iter().position(|&b| b == b'#') {
        Some(i) => &src[..i],
        None => src,
    }
}

/// Resolves an `<img src>` against the page URL, refusing a different
/// origin.
///
/// WHY same-origin only: the browser's capability grant is for a page a
/// person opened, not for every host that page mentions. A cross-origin
/// image is drawn as a placeholder and never requested.
pub fn resolve_same_origin(base: &[u8], src: &[u8], out: &mut [u8]) -> Result<usize, ResolveError> {
    let mut source = src;
    while let Some((&first, rest)) = source.split_first() {
        if first == b' ' || first == b'\t' || first == b'\n' || first == b'\r' {
            source = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = source.split_last() {
        if last == b' ' || last == b'\t' || last == b'\n' || last == b'\r' {
            source = rest;
        } else {
            break;
        }
    }
    source = split_fragment(source);
    if source.is_empty() || source == b"." {
        return Err(ResolveError::NoSource);
    }
    // Check if the source specifies a scheme (e.g. "data:", "blob:", "about:", "file:")
    // A scheme is [a-zA-Z][a-zA-Z0-9+.-]*: appearing before any '/', '?', or '#'
    if let Some(colon) = source.iter().position(|&b| b == b':' || b == b'/' || b == b'?' || b == b'#') {
        if source[colon] == b':' {
            if colon > 0
                && source[0].is_ascii_alphabetic()
                && source[..colon].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
            {
                if !source[colon..].starts_with(b"://") {
                    return Err(ResolveError::UnsupportedScheme);
                }
            }
        }
    }

    let base_uri = crate::parse_uri(base);
    if !matches!(base_uri.scheme, crate::UriScheme::Http | crate::UriScheme::Https) {
        return Err(ResolveError::UnsupportedScheme);
    }
    let Some(origin_end) = authority_end(base) else {
        return Err(ResolveError::InvalidBase);
    };
    let origin = &base[..origin_end];

    let mut resolved: [u8; 224] = [0u8; 224];
    let mut rn = 0usize;
    let mut write = |bytes: &[u8]| -> Result<(), ResolveError> {
        if rn + bytes.len() > resolved.len() {
            return Err(ResolveError::TooLong);
        }
        resolved[rn..rn + bytes.len()].copy_from_slice(bytes);
        rn += bytes.len();
        Ok(())
    };

    if let Some(scheme_end) = source.windows(3).position(|w| w == b"://") {
        let absolute = &source[..scheme_end + 3];
        let src_uri = crate::parse_uri(source);
        if crate::parse_uri(absolute).scheme != base_uri.scheme {
            return Err(ResolveError::CrossOrigin);
        }
        if !eq_ignore_ascii(src_uri.host, base_uri.host) || src_uri.port != base_uri.port {
            return Err(ResolveError::CrossOrigin);
        }
        write(source)?;
    } else if source.starts_with(b"//") {
        let rest = &source[2..];
        let end = rest
            .iter()
            .position(|&b| b == b'/' || b == b'?' || b == b'#')
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        let (host, port) = match authority.iter().rposition(|&b| b == b':') {
            Some(i) => {
                let digits = &authority[i + 1..];
                let mut value: u16 = 0;
                let mut ok = !digits.is_empty();
                for &b in digits {
                    if b.is_ascii_digit() {
                        value = value.saturating_mul(10).saturating_add((b - b'0') as u16);
                    } else {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    (&authority[..i], value)
                } else {
                    (authority, base_uri.scheme.default_port())
                }
            }
            None => (authority, base_uri.scheme.default_port()),
        };
        if !eq_ignore_ascii(host, base_uri.host) || port != base_uri.port {
            return Err(ResolveError::CrossOrigin);
        }
        // The origin is the base's scheme and authority, so only the
        // protocol-relative source's path and query are appended. Writing
        // the source whole would repeat the host inside the path.
        write(origin)?;
        let tail = &rest[end..];
        if tail.is_empty() {
            write(b"/")?;
        } else {
            if tail.starts_with(b"?") {
                write(b"/")?;
            }
            write(tail)?;
        }
    } else if source.starts_with(b"/") {
        write(origin)?;
        let (path, query) = match source.iter().position(|&b| b == b'?') {
            Some(i) => (&source[..i], Some(&source[i + 1..])),
            None => (source, None),
        };
        write(path)?;
        if let Some(q) = query {
            write(b"?")?;
            write(q)?;
        }
    } else if source.starts_with(b"?") {
        write(origin)?;
        let base_path = if base_uri.path.is_empty() { &b"/"[..] } else { base_uri.path };
        write(base_path)?;
        write(source)?;
    } else {
        write(origin)?;
        let base_path = if base_uri.path.is_empty() { &b"/"[..] } else { base_uri.path };
        let dir_end = base_path
            .iter()
            .rposition(|&b| b == b'/')
            .map(|i| i + 1)
            .unwrap_or(1);
        let dir = &base_path[..dir_end];
        let (src_path, query) = match source.iter().position(|&b| b == b'?') {
            Some(i) => (&source[..i], Some(&source[i + 1..])),
            None => (source, None),
        };
        let mut path: [u8; 160] = [0u8; 160];
        let mut pn = 0usize;
        for &b in dir {
            if pn >= path.len() {
                return Err(ResolveError::TooLong);
            }
            path[pn] = b;
            pn += 1;
        }
        // Normalise `.` and `..` segments. A `..` that would climb above
        // the root is clamped there, which is what a browser does.
        let mut cursor = 0usize;
        while cursor <= src_path.len() {
            let end = src_path[cursor..]
                .iter()
                .position(|&b| b == b'/')
                .map(|p| cursor + p)
                .unwrap_or(src_path.len());
            let segment = &src_path[cursor..end];
            if segment == b"." || segment.is_empty() {
                // Nothing to add.
            } else if segment == b".." {
                if pn > 1 {
                    let mut pop = pn - 1;
                    while pop > 0 && path[pop - 1] != b'/' {
                        pop -= 1;
                    }
                    pn = pop;
                }
            } else {
                if pn >= path.len() {
                    return Err(ResolveError::TooLong);
                }
                if pn > 0 && path[pn - 1] != b'/' {
                    if pn >= path.len() {
                        return Err(ResolveError::TooLong);
                    }
                    path[pn] = b'/';
                    pn += 1;
                }
                for &b in segment {
                    if pn >= path.len() {
                        return Err(ResolveError::TooLong);
                    }
                    path[pn] = b;
                    pn += 1;
                }
            }
            cursor = end.saturating_add(1);
        }
        write(&path[..pn])?;
        if let Some(q) = query {
            write(b"?")?;
            write(q)?;
        }
    }

    if rn > out.len() {
        return Err(ResolveError::TooLong);
    }
    out[..rn].copy_from_slice(&resolved[..rn]);
    Ok(rn)
}

/// The sentinel prefix of an image marker line.
pub const IMAGE_MARKER_PREFIX: &[u8] = b"\x01I";

/// An `<img>` element recorded in the reader document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImageMarker<'a> {
    pub src: &'a [u8],
    pub alt: &'a [u8],
    pub declared_w: Option<u16>,
    pub declared_h: Option<u16>,
}

/// Whether a reader-document line is an image marker.
pub fn is_image_marker(line: &[u8]) -> bool {
    line.starts_with(IMAGE_MARKER_PREFIX)
}

fn write_decimal(out: &mut [u8], mut n: usize, mut value: usize) -> usize {
    if value == 0 {
        if n < out.len() {
            out[n] = b'0';
            n += 1;
        }
        return n;
    }
    let mut digits = [0u8; 6];
    let mut len = 0usize;
    while value > 0 && len < digits.len() {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 && n < out.len() {
        len -= 1;
        out[n] = digits[len];
        n += 1;
    }
    n
}

fn write_ascii(out: &mut [u8], mut n: usize, bytes: &[u8], max: usize) -> usize {
    for &b in bytes.iter().take(max) {
        if n >= out.len() {
            break;
        }
        out[n] = if (0x20..=0x7E).contains(&b) && b != 0x1F { b } else { b'?' };
        n += 1;
    }
    n
}

/// Encodes an image marker line (no trailing newline).
///
/// Format: `0x01 'I' src 0x1F alt 0x1F width 0x1F height 0x02`, where a
/// missing dimension is `-`. A source the marker cannot carry exactly is
/// refused (returning zero), so the caller keeps the alt text instead of
/// fetching an address the page never named.
pub fn encode_image_marker(
    src: &[u8],
    alt: &[u8],
    declared_w: Option<u16>,
    declared_h: Option<u16>,
    out: &mut [u8],
) -> usize {
    let mut n = 0usize;
    if out.len() < 8 {
        return 0;
    }
    // A source that does not fit is not cut: a cut URL names a different
    // resource, and fetching it would put a picture the page never asked
    // for on the page. A byte the marker would rewrite is refused for the
    // same reason. The caller falls back to the alt text.
    if src.len() > 64
        || src
            .iter()
            .any(|&b| !(0x21..=0x7E).contains(&b) || b == 0x1F)
    {
        return 0;
    }
    if out.len() < 2 {
        return 0;
    }
    out[0] = IMAGE_MARKER_PREFIX[0];
    out[1] = IMAGE_MARKER_PREFIX[1];
    n += 2;
    n = write_ascii(out, n, src, 64);
    if n >= out.len() {
        return 0;
    }
    out[n] = 0x1F;
    n += 1;
    n = write_ascii(out, n, alt, 48);
    if n >= out.len() {
        return 0;
    }
    out[n] = 0x1F;
    n += 1;
    match declared_w {
        Some(w) if w > 0 => n = write_decimal(out, n, w as usize),
        _ => {
            if n >= out.len() {
                return 0;
            }
            out[n] = b'-';
            n += 1;
        }
    }
    if n >= out.len() {
        return 0;
    }
    out[n] = 0x1F;
    n += 1;
    match declared_h {
        Some(h) if h > 0 => n = write_decimal(out, n, h as usize),
        _ => {
            if n >= out.len() {
                return 0;
            }
            out[n] = b'-';
            n += 1;
        }
    }
    if n >= out.len() {
        return 0;
    }
    out[n] = 0x02;
    n += 1;
    n
}

fn parse_dimension(bytes: &[u8]) -> Option<Option<u16>> {
    if bytes == b"-" {
        return Some(None);
    }
    if bytes.is_empty() || bytes.len() > 5 {
        return None;
    }
    let mut value: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if value > u16::MAX as u32 {
        return None;
    }
    Some(Some(value as u16))
}

/// Parses one image marker line. A malformed marker yields `None`.
pub fn parse_image_marker(line: &[u8]) -> Option<ImageMarker<'_>> {
    if !is_image_marker(line) || line.last() != Some(&0x02) {
        return None;
    }
    let body = &line[2..line.len() - 1];
    let first = body.iter().position(|&b| b == 0x1F)?;
    let src = &body[..first];
    let rest = &body[first + 1..];
    let second = rest.iter().position(|&b| b == 0x1F)?;
    let alt = &rest[..second];
    let rest = &rest[second + 1..];
    let third = rest.iter().position(|&b| b == 0x1F)?;
    let declared_w_bytes = &rest[..third];
    let declared_h_bytes = &rest[third + 1..];
    if declared_h_bytes.iter().any(|&b| b == 0x1F) {
        return None;
    }
    let declared_w = parse_dimension(declared_w_bytes)?;
    let declared_h = parse_dimension(declared_h_bytes)?;
    Some(ImageMarker {
        src,
        alt,
        declared_w,
        declared_h,
    })
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec;

    /// A valid 1x1 RGBA PNG header (IHDR included, CRC correct).
    const PNG_1X1: [u8; 33] = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89,
    ];

    #[test]
    fn an_oversized_image_is_refused_with_a_typed_outcome() {
        let body = vec![0u8; MAX_IMAGE_BODY_BYTES + 1];
        assert_eq!(
            admit_image(&body),
            ImageAdmission::Refused(ImageRefusal::TooLarge {
                bytes: MAX_IMAGE_BODY_BYTES + 1,
                limit: MAX_IMAGE_BODY_BYTES,
            })
        );
    }

    #[test]
    fn an_unsupported_type_yields_an_unsupported_outcome_with_a_placeholder() {
        let body = b"<html>this is not an image</html>";
        assert_eq!(sniff_image_type(body), ImageType::Unsupported);
        assert_eq!(
            admit_image(body),
            ImageAdmission::Refused(ImageRefusal::UnsupportedType)
        );
        assert!(ImageRefusal::UnsupportedType
            .placeholder()
            .starts_with(b"Image not shown:"));
    }

    #[test]
    fn a_lossy_webp_is_named_unsupported_not_corrupt() {
        let mut body = [0u8; 32];
        body[..4].copy_from_slice(b"RIFF");
        body[8..12].copy_from_slice(b"WEBP");
        body[12..16].copy_from_slice(b"VP8 ");
        assert_eq!(sniff_image_type(&body), ImageType::WebP);
        assert_eq!(
            admit_image(&body),
            ImageAdmission::Refused(ImageRefusal::UnsupportedType)
        );
    }

    #[test]
    fn a_png_header_is_admitted_with_its_dimensions() {
        assert_eq!(sniff_image_type(&PNG_1X1), ImageType::Png);
        assert_eq!(
            admit_image(&PNG_1X1),
            ImageAdmission::Accepted {
                kind: ImageType::Png,
                width: 1,
                height: 1,
            }
        );
    }

    #[test]
    fn a_header_that_declares_too_many_pixels_is_refused_before_decoding() {
        assert!(check_dimensions(4096, 4096).is_err());
        assert!(check_dimensions(0, 10).is_err());
        assert_eq!(check_dimensions(512, 512), Ok((512, 512)));
        assert_eq!(
            check_dimensions(600, 10),
            Err(ImageRefusal::DimensionsTooLarge {
                width: 600,
                height: 10,
                limit: MAX_IMAGE_DIMENSION,
            })
        );
    }

    #[test]
    fn the_rgba_requirement_is_checked() {
        assert_eq!(required_rgba_bytes(512, 512), Some(1_048_576));
        assert_eq!(required_rgba_bytes(0, 0), Some(0));
    }

    #[test]
    fn display_size_follows_declarations_and_the_column() {
        assert_eq!(fit_display(100, 50, None, None), (100, 50));
        assert_eq!(fit_display(100, 50, Some(200), None), (200, 100));
        assert_eq!(fit_display(100, 50, None, Some(100)), (200, 100));
        assert_eq!(fit_display(2000, 1000, None, None), (MAX_DISPLAY_WIDTH, 336));
        assert_eq!(fit_display(1, 5000, None, None), (1, MAX_DISPLAY_HEIGHT));
    }

    #[test]
    fn an_image_marker_round_trips() {
        let mut buf = [0u8; 160];
        let n = encode_image_marker(b"pic.png", b"a picture", Some(120), None, &mut buf);
        let marker = parse_image_marker(&buf[..n]).expect("marker parses");
        assert_eq!(marker.src, b"pic.png");
        assert_eq!(marker.alt, b"a picture");
        assert_eq!(marker.declared_w, Some(120));
        assert_eq!(marker.declared_h, None);

        let n = encode_image_marker(b"x", b"", None, Some(44), &mut buf);
        let marker = parse_image_marker(&buf[..n]).unwrap();
        assert_eq!(marker.declared_h, Some(44));

        assert_eq!(parse_image_marker(b"plain text"), None);
        assert_eq!(parse_image_marker(b"\x01Igarbage"), None);
        // Extra separator field or non-digit dimension must be rejected
        assert_eq!(parse_image_marker(b"\x01Isrc\x1Falt\x1F10\x1F20\x1Fextra\x02"), None);
        assert_eq!(parse_image_marker(b"\x01Isrc\x1Falt\x1Fabc\x1F20\x02"), None);
    }

    #[test]
    fn small_buffer_encodes_no_marker() {
        let mut small = [0u8; 12];
        assert_eq!(encode_image_marker(b"image.png", b"alt", Some(100), Some(200), &mut small), 0);
    }

    #[test]
    fn non_hierarchical_schemes_are_unsupported() {
        let mut out = [0u8; 224];
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"blob:https://example.com/uuid", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"about:blank", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"file:/local/path", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
    }

    #[test]
    fn a_source_too_long_to_carry_is_not_cut() {
        let mut buf = [0u8; 160];
        let long = [b'a'; 65];
        assert_eq!(encode_image_marker(&long, b"alt", None, None, &mut buf), 0);
        let longest = [b'b'; 64];
        assert!(encode_image_marker(&longest, b"alt", None, None, &mut buf) > 0);
    }

    #[test]
    fn a_relative_source_resolves_against_the_page() {
        let mut out = [0u8; 224];
        let n = resolve_same_origin(
            b"https://example.com/docs/page.html",
            b"../img/logo.png",
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[..n], b"https://example.com/img/logo.png");

        let n = resolve_same_origin(b"https://example.com/a/b", b"/x.png", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/x.png");

        let n = resolve_same_origin(b"https://example.com/a/b", b"pic.png", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/a/pic.png");
    }

    #[test]
    fn a_cross_origin_source_is_refused() {
        let mut out = [0u8; 224];
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"https://cdn.example.net/x.png", &mut out),
            Err(ResolveError::CrossOrigin)
        );
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"//cdn.example.net/x.png", &mut out),
            Err(ResolveError::CrossOrigin)
        );
        assert_eq!(
            resolve_same_origin(b"https://example.com/a", b"data:image/png;base64,AA", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
    }

    #[test]
    fn a_protocol_relative_source_on_the_same_origin_resolves() {
        let mut out = [0u8; 224];
        let n = resolve_same_origin(b"https://example.com/a", b"//example.com/x.png", &mut out)
            .unwrap();
        assert_eq!(&out[..n], b"https://example.com/x.png");
    }
}
