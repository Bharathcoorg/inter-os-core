//! Bounded inline-image policy for the native reader.
//!
//! This module answers every question about an image that can be answered
//! without a network or a decoder: how large a body is acceptable, what
//! the bytes claim to be, what dimensions the header declares, whether
//! that is inside the decoder's budget, and what an honest placeholder
//! says when the answer is no.
//!
//! It also carries the reader-document marker that records an `<img>`
//! element and the URL resolution the browser uses before it will fetch
//! an image.
//!
//! The policy is deliberately narrow:
//! - bodies above [`MAX_IMAGE_BODY_BYTES`] are refused, never truncated
//!   into a half-image;
//! - only PNG, JPEG, GIF, WebP and SVG are accepted, because those
//!   are the decoders the platform actually has;
//! - a declared width or height above [`MAX_IMAGE_DIMENSION`] is refused
//!   before a byte of RGBA is allocated;
//! - an image is fetched from whatever http(s) address the page names,
//!   including another host, because a page's pictures are the page's
//!   content. `data:` addresses are not decoded and say so.

/// The largest image body the browser will accept.
///
/// WHY this cap: an inline picture is decoded into a static arena and
/// drawn in a scrolling column, and the transport that carries it holds a
/// quarter of a megabyte, so a body is only whole while it fits under
/// that. The response is refused rather than cut, because half a PNG is
/// not a PNG.
pub const MAX_IMAGE_BODY_BYTES: usize = 224 * 1024;

/// The largest width or height an inline image may declare.
///
/// WHY this number: it is the page's own width, so a hero image can be
/// shown at the size the page drew it, and nothing wider can be displayed
/// without scaling it down first.
pub const MAX_IMAGE_DIMENSION: u32 = 720;

/// The most pixels one inline image may declare.
pub const MAX_IMAGE_PIXELS: usize = (MAX_IMAGE_DIMENSION as usize) * (MAX_IMAGE_DIMENSION as usize);

/// How many inline images a page may place.
pub const MAX_INLINE_IMAGES: usize = 8;

/// The decoded-pixel arena, in bytes.
///
/// WHY a shared arena: eight images at the per-image cap would be
/// twenty-three megabytes. A single arena bounds the total a page can make
/// the browser hold, and an image that does not fit is reported as refused
/// rather than silently dropped.
pub const IMAGE_ARENA_BYTES: usize = 4 * 1024 * 1024;

/// The widest an inline image is displayed, in logical pixels.
pub const MAX_DISPLAY_WIDTH: usize = 672;

/// The longest source address the browser will carry to the fetcher.
///
/// WHY this is a name and not a literal in two places: a content host's
/// address is routinely longer than a path on the page's own host, so the
/// copy that hands a source to the resolver has to be at least as wide as
/// the buffer the resolved address lands in. Anything longer is refused
/// as what it is rather than fetched cut short, which would be asking for
/// different bytes than the page named.
pub const MAX_SRC_LEN: usize = 224;

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
    /// A Windows icon container: a directory of pictures, each a PNG or a
    /// device-independent bitmap, which is the form a page most often names
    /// its own picture in.
    Ico,
    /// A scalable vector graphic.
    Svg,
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
    /// The document on screen arrived over a verified TLS channel and this
    /// reader carries no subresource over TLS, so the picture was blocked as
    /// mixed content rather than fetched in the open.
    InsecureSubresource,
    /// The body (or the declared length) exceeded the cap.
    TooLarge { bytes: usize, limit: usize },
    /// The bytes are not one of the supported formats (PNG, JPEG, GIF, WebP, SVG, ICO).
    UnsupportedType,
    /// The header declared more pixels than the reader allows.
    DimensionsTooLarge { width: u32, height: u32, limit: u32 },
    /// The header could not be read, or names an unsupported variant
    /// (progressive JPEG, 16-bit PNG).
    BadHeader,
    /// The decoder refused the body after it was admitted.
    DecodeFailed,
    /// The page is a local document, so no network fetch is made.
    LocalDocument,
    /// The decoder or the page's arena is full.
    MemoryBudget,
    /// The response did not arrive, or was not a successful one.
    FetchFailed { code: i32 },
    /// The reply stopped short of the file's own length, so the bytes in
    /// hand are the beginning of a picture and not a picture.
    CutShort { bytes: usize },
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
            ImageRefusal::InsecureSubresource => b"Image not shown: plaintext on a verified page",
            ImageRefusal::TooLarge { .. } => b"Image not shown: too large",
            ImageRefusal::UnsupportedType => b"Image not shown: unsupported type",
            ImageRefusal::DimensionsTooLarge { .. } => b"Image not shown: dimensions too large",
            ImageRefusal::BadHeader => b"Image not shown: unsupported format variant",
            ImageRefusal::DecodeFailed => b"Image not shown: decode failed",
            ImageRefusal::LocalDocument => b"Image not shown: local document",
            ImageRefusal::MemoryBudget => b"Image not shown: memory budget",
            ImageRefusal::FetchFailed { .. } => b"Image not shown: fetch failed",
            ImageRefusal::CutShort { .. } => b"Image not shown: reply cut short",
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

/// The reader's one mixed-content rule, asked of it once per subresource.
///
/// WHY a verified document blocks a plaintext subresource: the padlock in the
/// address field is a claim about the channel that delivered the document, and
/// #83 pinned it to exactly that observation. A page carrying that mark whose
/// pictures and stylesheets arrive over an unencrypted hop is not the page the
/// mark describes -- anything on that path can rewrite them, and the person
/// reading the lock has no way to see it. The honest options were to stop
/// claiming the page is protected or to stop fetching it in the open, and the
/// browser keeps its claim by refusing the fetch and drawing why.
///
/// WHY the rule runs in one direction only: a document that itself arrived over
/// the same plaintext hop is claiming no protection, so blocking its
/// subresources would report a security the page never had and would break the
/// gateway pages this reader is built for. The refusal is a downgrade of a
/// promise, and only a verified document made one.
///
/// WHY the caller passes the transport and not the address: every subresource
/// this reader fetches -- inline image, page icon, linked stylesheet -- goes out
/// on the plaintext gateway hop, whatever the URL says, because each of those
/// sites calls its transport with `use_tls` false. A subresource addressed
/// `https://` is no more encrypted here than one addressed `http://`, so the
/// scheme cannot be the test and the transport has to be.
pub fn mixed_content_block(document_tls_verified: bool, subresource_is_plaintext: bool) -> bool {
    document_tls_verified && subresource_is_plaintext
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
    // An icon container says what it is with two zero bytes, a type of one
    // and a non-empty directory. The signature is weaker than the others
    // here, so it is only the opening: `decode_ico_header` walks the
    // directory and refuses the file if none of its entries is a picture.
    if body.len() >= 22 && body[..4] == [0, 0, 1, 0] && body[4..6] != [0, 0] {
        return ImageType::Ico;
    }
    if crate::std::image::is_svg(body) {
        return ImageType::Svg;
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
            image::decode_webp_header(body).map_err(|_| ImageRefusal::BadHeader)
        }
        ImageType::Ico => image::decode_ico_header(body).map_err(|_| ImageRefusal::BadHeader),
        ImageType::Svg => image::decode_svg_header(body).map_err(|_| ImageRefusal::BadHeader),
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
    UnsupportedScheme,
    InvalidBase,
    TooLong,
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

/// Resolves an `<img src>`, `<link href>` or other subresource address
/// against the page URL.
///
/// WHY another host is allowed: a page's pictures and stylesheets live
/// wherever its author put them, and refusing the ones on a content host
/// left the reader with a page of holes. The request is still only ever
/// made through the gateway, carries no cookie for the page it was
/// browsing from, and is refused outright for schemes this transport does
/// not speak.
pub fn resolve_subresource(base: &[u8], src: &[u8], out: &mut [u8]) -> Result<usize, ResolveError> {
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

    if source.windows(3).any(|w| w == b"://") {
        // An absolute address is used as the page wrote it, on this host or
        // another. Only the two schemes the transport speaks get through.
        if !matches!(
            crate::parse_uri(source).scheme,
            crate::UriScheme::Http | crate::UriScheme::Https
        ) {
            return Err(ResolveError::UnsupportedScheme);
        }
        write(source)?;
    } else if source.starts_with(b"//") {
        let rest = &source[2..];
        let end = rest
            .iter()
            .position(|&b| b == b'/' || b == b'?' || b == b'#')
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        if authority.is_empty() {
            return Err(ResolveError::NoSource);
        }
        // A protocol-relative address borrows the page's scheme and keeps
        // the host it names, which need not be the page's own. Writing the
        // source whole would drop the scheme; writing the page's origin
        // would replace the host the page asked for.
        let scheme: &[u8] = if matches!(base_uri.scheme, crate::UriScheme::Https) {
            b"https://"
        } else {
            b"http://"
        };
        write(scheme)?;
        write(authority)?;
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
    fn a_lossy_webp_is_admitted() {
        let mut body = [0u8; 30];
        body[..4].copy_from_slice(b"RIFF");
        body[4..8].copy_from_slice(&22u32.to_le_bytes());
        body[8..12].copy_from_slice(b"WEBP");
        body[12..16].copy_from_slice(b"VP8 ");
        body[16..20].copy_from_slice(&10u32.to_le_bytes());
        body[20..23].copy_from_slice(&[0x10, 0x00, 0x00]);
        body[23..26].copy_from_slice(&[0x9D, 0x01, 0x2A]);
        body[26..28].copy_from_slice(&64u16.to_le_bytes());
        body[28..30].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(sniff_image_type(&body), ImageType::WebP);
        assert_eq!(
            admit_image(&body),
            ImageAdmission::Accepted {
                kind: ImageType::WebP,
                width: 64,
                height: 64,
            }
        );
    }

    #[test]
    fn an_svg_is_admitted() {
        let body = b"<svg width=\"48\" height=\"48\" viewBox=\"0 0 48 48\"><rect width=\"48\" height=\"48\" fill=\"red\"/></svg>";
        assert_eq!(sniff_image_type(body), ImageType::Svg);
        assert_eq!(
            admit_image(body),
            ImageAdmission::Accepted {
                kind: ImageType::Svg,
                width: 48,
                height: 48,
            }
        );
    }

    /// A one-entry icon container around a 2x2 thirty-two-bit bitmap: the
    /// header, the directory, and the bitmap whose height counts the pixels
    /// and the mask under them.
    fn icon_two_by_two() -> std::vec::Vec<u8> {
        let mut body = vec![0u8; 6 + 16 + 40 + 16];
        body[..4].copy_from_slice(&[0, 0, 1, 0]);
        body[4..6].copy_from_slice(&1u16.to_le_bytes());
        body[6] = 2;
        body[7] = 2;
        body[10..12].copy_from_slice(&1u16.to_le_bytes());
        body[12..14].copy_from_slice(&32u16.to_le_bytes());
        body[14..18].copy_from_slice(&56u32.to_le_bytes());
        body[18..22].copy_from_slice(&22u32.to_le_bytes());
        body[22..26].copy_from_slice(&40u32.to_le_bytes());
        body[26..30].copy_from_slice(&2i32.to_le_bytes());
        body[30..34].copy_from_slice(&4i32.to_le_bytes());
        body[34..36].copy_from_slice(&1u16.to_le_bytes());
        body[36..38].copy_from_slice(&32u16.to_le_bytes());
        body
    }

    #[test]
    fn an_icon_container_is_admitted_at_the_size_of_its_picture() {
        let body = icon_two_by_two();
        assert_eq!(sniff_image_type(&body), ImageType::Ico);
        assert_eq!(
            admit_image(&body),
            ImageAdmission::Accepted {
                kind: ImageType::Ico,
                width: 2,
                height: 2,
            }
        );
    }

    #[test]
    fn a_file_that_only_rhymes_with_an_icon_header_is_not_admitted() {
        // The signature is four bytes; the directory behind it has to name a
        // picture that is actually in the file. This one points its only
        // entry at its own header, and the reader says so rather than
        // drawing whatever those bytes happen to look like.
        let mut body = icon_two_by_two();
        body[18..22].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(sniff_image_type(&body), ImageType::Ico);
        assert_eq!(
            admit_image(&body),
            ImageAdmission::Refused(ImageRefusal::BadHeader)
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
        let limit = MAX_IMAGE_DIMENSION;
        assert!(check_dimensions(4096, 4096).is_err());
        assert!(check_dimensions(0, 10).is_err());
        assert_eq!(check_dimensions(limit, limit), Ok((limit, limit)));
        assert_eq!(
            check_dimensions(limit + 1, 10),
            Err(ImageRefusal::DimensionsTooLarge {
                width: limit + 1,
                height: 10,
                limit,
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
            resolve_subresource(b"https://example.com/a", b"blob:https://example.com/uuid", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
        assert_eq!(
            resolve_subresource(b"https://example.com/a", b"about:blank", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
        assert_eq!(
            resolve_subresource(b"https://example.com/a", b"file:/local/path", &mut out),
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
        let n = resolve_subresource(
            b"https://example.com/docs/page.html",
            b"../img/logo.png",
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[..n], b"https://example.com/img/logo.png");

        let n = resolve_subresource(b"https://example.com/a/b", b"/x.png", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/x.png");

        let n = resolve_subresource(b"https://example.com/a/b", b"pic.png", &mut out).unwrap();
        assert_eq!(&out[..n], b"https://example.com/a/pic.png");
    }

    #[test]
    fn a_source_on_another_host_is_resolved_rather_than_refused() {
        let mut out = [0u8; 224];
        let n = resolve_subresource(
            b"https://example.com/a",
            b"https://cdn.example.net/x.png",
            &mut out,
        )
        .unwrap();
        assert_eq!(&out[..n], b"https://cdn.example.net/x.png");
        // A protocol-relative address borrows the page's scheme, and keeps
        // the host it names for itself.
        let n = resolve_subresource(b"https://example.com/a", b"//cdn.example.net/x.png", &mut out)
            .unwrap();
        assert_eq!(&out[..n], b"https://cdn.example.net/x.png");
        let n = resolve_subresource(b"http://example.com/a", b"//cdn.example.net/x.png", &mut out)
            .unwrap();
        assert_eq!(&out[..n], b"http://cdn.example.net/x.png");
        let n = resolve_subresource(b"https://example.com/a", b"//example.com:8443/x.png", &mut out)
            .unwrap();
        assert_eq!(&out[..n], b"https://example.com:8443/x.png");
    }

    #[test]
    fn a_source_that_names_nothing_fetchable_is_refused_for_its_own_reason() {
        let mut out = [0u8; 224];
        assert_eq!(
            resolve_subresource(b"https://example.com/a", b"//", &mut out),
            Err(ResolveError::NoSource)
        );
        assert_eq!(
            resolve_subresource(b"https://example.com/a", b"data:image/png;base64,AA", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
        assert_eq!(
            resolve_subresource(b"https://example.com/a", b"ftp://host/x.png", &mut out),
            Err(ResolveError::UnsupportedScheme)
        );
    }

    /// The whole mixed-content table, in both directions.
    ///
    /// WHY all four corners are asserted: a rule stated as
    /// `document_tls_verified` alone passes today's suite and silently blocks
    /// every gateway page's pictures, and a rule stated as
    /// `subresource_is_plaintext` alone blocks nothing on the page that needs
    /// blocking. The refusal is a downgrade of a promise, so both halves have
    /// to be present and both have to be tested.
    #[test]
    fn only_a_verified_document_blocks_a_plaintext_subresource() {
        assert!(mixed_content_block(true, true));
        assert!(!mixed_content_block(false, true));
        assert!(!mixed_content_block(false, false));
        assert!(!mixed_content_block(true, false));
    }

    #[test]
    fn a_blocked_subresource_is_refused_for_its_own_reason_and_says_so() {
        // The reason is its own and not borrowed from a fetch that failed: the
        // transport was never asked, and a placeholder that said "fetch failed"
        // would send the reader to a gateway that was never dialled.
        assert_ne!(
            ImageRefusal::InsecureSubresource,
            ImageRefusal::FetchFailed { code: -5 }
        );
        assert_ne!(
            ImageRefusal::InsecureSubresource.placeholder(),
            ImageRefusal::FetchFailed { code: -5 }.placeholder()
        );
        assert!(ImageRefusal::InsecureSubresource
            .placeholder()
            .starts_with(b"Image not shown:"));
        // The card is measured from this text and drawn beside the address
        // field's own mark, so the words have to fit the page's column.
        assert!(ImageRefusal::InsecureSubresource.placeholder().len() <= 48);
    }
}
