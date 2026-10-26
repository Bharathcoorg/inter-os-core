//! Zero-heap bounded album art parser for MP3 ID3v2 APIC and FLAC METADATA_BLOCK_PICTURE.
//!
//! Extracts embedded JPEG or PNG cover art up to 32 KiB into a caller-provided buffer.
//! Strictly `#![no_std]` with zero heap allocations.

/// Maximum size of embedded album art accepted (32 KiB).
pub const MAX_ART_BYTES: usize = 32 * 1024;

/// Errors returned by the album art extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtError {
    /// No album art was found in the media container.
    NoArt,
    /// The embedded image exceeds the 32 KiB bounded size limit.
    TooLarge,
    /// The image data or container metadata is corrupt or invalid.
    Corrupt,
    /// The container or image format is unsupported.
    UnsupportedFormat,
    /// Data buffer was truncated before the full frame could be read.
    Truncated,
}

/// Identifies if the raw byte slice begins with a valid JPEG magic signature (`0xFF, 0xD8, 0xFF`).
#[inline]
pub fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

/// Identifies if the raw byte slice begins with a valid PNG magic signature (`0x89, 'P', 'N', 'G'`).
#[inline]
pub fn is_png(data: &[u8]) -> bool {
    data.len() >= 4 && data[0] == 0x89 && data[1] == b'P' && data[2] == b'N' && data[3] == b'G'
}

/// Parses an ID3v2 APIC frame from an MP3 byte slice and extracts the raw JPEG/PNG image data into `out`.
///
/// Bounded to `MAX_ART_BYTES` (32 KiB). Returns `Ok(bytes_written)` or an `ArtError`.
pub fn extract_id3v2_apic(data: &[u8], out: &mut [u8]) -> Result<usize, ArtError> {
    if data.len() < 10 {
        return Err(ArtError::Truncated);
    }
    if &data[0..3] != b"ID3" {
        return Err(ArtError::UnsupportedFormat);
    }

    let ver_major = data[3];
    if ver_major < 2 || ver_major > 4 {
        return Err(ArtError::UnsupportedFormat);
    }

    let flags = data[5];
    let tag_size = ((data[6] as usize & 0x7F) << 21)
        | ((data[7] as usize & 0x7F) << 14)
        | ((data[8] as usize & 0x7F) << 7)
        | (data[9] as usize & 0x7F);

    let tag_limit = 10 + tag_size;
    let tag_end = tag_limit.min(data.len());
    let mut cursor = 10;

    // Check for extended header flag (bit 6 of flags in v2.3 / v2.4)
    if (flags & 0x40) != 0 && cursor + 4 <= tag_end {
        let ext_size = if ver_major == 4 {
            ((data[cursor] as usize & 0x7F) << 21)
                | ((data[cursor + 1] as usize & 0x7F) << 14)
                | ((data[cursor + 2] as usize & 0x7F) << 7)
                | (data[cursor + 3] as usize & 0x7F)
        } else {
            ((data[cursor] as usize) << 24)
                | ((data[cursor + 1] as usize) << 16)
                | ((data[cursor + 2] as usize) << 8)
                | (data[cursor + 3] as usize)
        };
        cursor += ext_size.max(4);
    }

    while cursor + 10 <= tag_end {
        // Zero byte indicates start of padding
        if data[cursor] == 0 {
            break;
        }

        let frame_id = &data[cursor..cursor + 4];
        let frame_size = if ver_major == 4 {
            ((data[cursor + 4] as usize & 0x7F) << 21)
                | ((data[cursor + 5] as usize & 0x7F) << 14)
                | ((data[cursor + 6] as usize & 0x7F) << 7)
                | (data[cursor + 7] as usize & 0x7F)
        } else {
            ((data[cursor + 4] as usize) << 24)
                | ((data[cursor + 5] as usize) << 16)
                | ((data[cursor + 6] as usize) << 8)
                | (data[cursor + 7] as usize)
        };

        cursor += 10;
        if cursor + frame_size > data.len() {
            return Err(ArtError::Truncated);
        }
        let frame_data = &data[cursor..cursor + frame_size];
        cursor += frame_size;

        if frame_id == b"APIC" {
            if frame_data.len() < 4 {
                return Err(ArtError::Corrupt);
            }
            let encoding = frame_data[0];

            // Scan null terminator for MIME string
            let mut mime_end = 1;
            while mime_end < frame_data.len() && frame_data[mime_end] != 0 {
                mime_end += 1;
            }
            if mime_end >= frame_data.len() {
                return Err(ArtError::Corrupt);
            }

            // Skip picture type byte (mime_end + 1)
            let desc_start = mime_end + 2;
            if desc_start > frame_data.len() {
                return Err(ArtError::Corrupt);
            }

            // Scan description string terminator based on text encoding
            let pic_start = if encoding == 0 || encoding == 3 {
                // ISO-8859-1 or UTF-8: single null byte terminator
                let mut desc_end = desc_start;
                while desc_end < frame_data.len() && frame_data[desc_end] != 0 {
                    desc_end += 1;
                }
                if desc_end >= frame_data.len() {
                    return Err(ArtError::Corrupt);
                }
                desc_end + 1
            } else {
                // UTF-16 with BOM or UTF-16BE: double null byte terminator
                let mut desc_end = desc_start;
                let mut found = false;
                while desc_end + 1 < frame_data.len() {
                    if frame_data[desc_end] == 0 && frame_data[desc_end + 1] == 0 {
                        found = true;
                        break;
                    }
                    desc_end += 2;
                }
                if !found {
                    return Err(ArtError::Corrupt);
                }
                desc_end + 2
            };

            if pic_start >= frame_data.len() {
                return Err(ArtError::Corrupt);
            }

            let pic_data = &frame_data[pic_start..];
            if !is_jpeg(pic_data) && !is_png(pic_data) {
                return Err(ArtError::Corrupt);
            }

            if pic_data.len() > MAX_ART_BYTES {
                return Err(ArtError::TooLarge);
            }
            if pic_data.len() > out.len() {
                return Err(ArtError::TooLarge);
            }

            out[..pic_data.len()].copy_from_slice(pic_data);
            return Ok(pic_data.len());
        }
    }

    Err(ArtError::NoArt)
}

/// Parses a METADATA_BLOCK_PICTURE from a FLAC byte slice and extracts the raw JPEG/PNG image data into `out`.
///
/// Bounded to `MAX_ART_BYTES` (32 KiB). Returns `Ok(bytes_written)` or an `ArtError`.
pub fn extract_flac_picture(data: &[u8], out: &mut [u8]) -> Result<usize, ArtError> {
    if data.len() < 4 {
        return Err(ArtError::Truncated);
    }
    if &data[0..4] != b"fLaC" {
        return Err(ArtError::UnsupportedFormat);
    }

    let mut cursor = 4;
    while cursor + 4 <= data.len() {
        let header_byte = data[cursor];
        let is_last = (header_byte & 0x80) != 0;
        let block_type = header_byte & 0x7F;
        let block_len = ((data[cursor + 1] as usize) << 16)
            | ((data[cursor + 2] as usize) << 8)
            | (data[cursor + 3] as usize);

        cursor += 4;
        if cursor + block_len > data.len() {
            return Err(ArtError::Truncated);
        }
        let block_data = &data[cursor..cursor + block_len];
        cursor += block_len;

        // Block type 6 is METADATA_BLOCK_PICTURE
        if block_type == 6 {
            if block_data.len() < 32 {
                return Err(ArtError::Corrupt);
            }

            // Offset 4: MIME type length (u32 BE)
            let mime_len = u32::from_be_bytes(block_data[4..8].try_into().map_err(|_| ArtError::Corrupt)?) as usize;
            let desc_offset = 8 + mime_len;
            if desc_offset + 4 > block_data.len() {
                return Err(ArtError::Corrupt);
            }

            // Description length (u32 BE)
            let desc_len = u32::from_be_bytes(block_data[desc_offset..desc_offset + 4].try_into().map_err(|_| ArtError::Corrupt)?) as usize;
            let fields_offset = desc_offset + 4 + desc_len;

            // 16 bytes for width(4), height(4), depth(4), colors(4), then picture data length (4)
            let data_len_offset = fields_offset + 16;
            if data_len_offset + 4 > block_data.len() {
                return Err(ArtError::Corrupt);
            }

            let data_len = u32::from_be_bytes(block_data[data_len_offset..data_len_offset + 4].try_into().map_err(|_| ArtError::Corrupt)?) as usize;
            let pic_start = data_len_offset + 4;
            if pic_start + data_len > block_data.len() {
                return Err(ArtError::Corrupt);
            }

            let pic_data = &block_data[pic_start..pic_start + data_len];
            if !is_jpeg(pic_data) && !is_png(pic_data) {
                return Err(ArtError::Corrupt);
            }

            if pic_data.len() > MAX_ART_BYTES {
                return Err(ArtError::TooLarge);
            }
            if pic_data.len() > out.len() {
                return Err(ArtError::TooLarge);
            }

            out[..pic_data.len()].copy_from_slice(pic_data);
            return Ok(pic_data.len());
        }

        if is_last {
            break;
        }
    }

    Err(ArtError::NoArt)
}

/// Sniffs container format and attempts to extract album art.
pub fn extract_album_art(data: &[u8], out: &mut [u8]) -> Result<usize, ArtError> {
    if data.len() >= 3 && &data[0..3] == b"ID3" {
        extract_id3v2_apic(data, out)
    } else if data.len() >= 4 && &data[0..4] == b"fLaC" {
        extract_flac_picture(data, out)
    } else {
        Err(ArtError::NoArt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_id3v2_apic_jpeg() {
        // Construct minimal valid ID3v2.3 tag with APIC frame holding JPEG
        let jpeg_payload = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
        let mut apic_body = [0u8; 64];
        let mut idx = 0;
        apic_body[idx] = 0; // ISO-8859-1 encoding
        idx += 1;
        let mime = b"image/jpeg\0";
        apic_body[idx..idx + mime.len()].copy_from_slice(mime);
        idx += mime.len();
        apic_body[idx] = 3; // Cover (front)
        idx += 1;
        apic_body[idx] = 0; // Empty description null terminator
        idx += 1;
        apic_body[idx..idx + jpeg_payload.len()].copy_from_slice(&jpeg_payload);
        idx += jpeg_payload.len();

        let frame_size = idx as u32;
        let mut tag = [0u8; 128];
        tag[0..3].copy_from_slice(b"ID3");
        tag[3] = 3; // v2.3
        tag[4] = 0;
        tag[5] = 0;
        // tag size synchsafe
        let total_tag_len = (10 + frame_size) as usize;
        tag[6] = ((total_tag_len >> 21) & 0x7F) as u8;
        tag[7] = ((total_tag_len >> 14) & 0x7F) as u8;
        tag[8] = ((total_tag_len >> 7) & 0x7F) as u8;
        tag[9] = (total_tag_len & 0x7F) as u8;

        // Frame header
        tag[10..14].copy_from_slice(b"APIC");
        tag[14..18].copy_from_slice(&frame_size.to_be_bytes());
        tag[18] = 0;
        tag[19] = 0;
        tag[20..20 + idx].copy_from_slice(&apic_body[..idx]);

        let mut out = [0u8; MAX_ART_BYTES];
        let n = extract_id3v2_apic(&tag[..20 + idx], &mut out).expect("JPEG APIC should be extracted");
        assert_eq!(n, jpeg_payload.len());
        assert_eq!(&out[..n], &jpeg_payload);
    }

    #[test]
    fn test_extract_id3v2_apic_png() {
        let png_payload = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut apic_body = [0u8; 64];
        let mut idx = 0;
        apic_body[idx] = 3; // UTF-8 encoding
        idx += 1;
        let mime = b"image/png\0";
        apic_body[idx..idx + mime.len()].copy_from_slice(mime);
        idx += mime.len();
        apic_body[idx] = 3; // Cover (front)
        idx += 1;
        apic_body[idx] = 0; // Empty description
        idx += 1;
        apic_body[idx..idx + png_payload.len()].copy_from_slice(&png_payload);
        idx += png_payload.len();

        let frame_size = idx as u32;
        let mut tag = [0u8; 128];
        tag[0..3].copy_from_slice(b"ID3");
        tag[3] = 4; // v2.4
        tag[4] = 0;
        tag[5] = 0;
        let total_tag_len = (10 + frame_size) as usize;
        tag[6] = ((total_tag_len >> 21) & 0x7F) as u8;
        tag[7] = ((total_tag_len >> 14) & 0x7F) as u8;
        tag[8] = ((total_tag_len >> 7) & 0x7F) as u8;
        tag[9] = (total_tag_len & 0x7F) as u8;

        tag[10..14].copy_from_slice(b"APIC");
        // v2.4 synchsafe frame size
        tag[14] = ((frame_size >> 21) & 0x7F) as u8;
        tag[15] = ((frame_size >> 14) & 0x7F) as u8;
        tag[16] = ((frame_size >> 7) & 0x7F) as u8;
        tag[17] = (frame_size & 0x7F) as u8;
        tag[18] = 0;
        tag[19] = 0;
        tag[20..20 + idx].copy_from_slice(&apic_body[..idx]);

        let mut out = [0u8; MAX_ART_BYTES];
        let n = extract_id3v2_apic(&tag[..20 + idx], &mut out).expect("PNG APIC should be extracted");
        assert_eq!(n, png_payload.len());
        assert_eq!(&out[..n], &png_payload);
    }

    #[test]
    fn test_extract_flac_picture_jpeg() {
        let jpeg_payload = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let mut block = [0u8; 128];
        let mut idx = 0;
        // Picture type: 3 (Cover front)
        block[idx..idx + 4].copy_from_slice(&3u32.to_be_bytes());
        idx += 4;
        // MIME length: 10
        let mime = b"image/jpeg";
        block[idx..idx + 4].copy_from_slice(&(mime.len() as u32).to_be_bytes());
        idx += 4;
        block[idx..idx + mime.len()].copy_from_slice(mime);
        idx += mime.len();
        // Desc length: 0
        block[idx..idx + 4].copy_from_slice(&0u32.to_be_bytes());
        idx += 4;
        // Width: 100, Height: 100, Depth: 24, Colors: 0
        block[idx..idx + 4].copy_from_slice(&100u32.to_be_bytes());
        idx += 4;
        block[idx..idx + 4].copy_from_slice(&100u32.to_be_bytes());
        idx += 4;
        block[idx..idx + 4].copy_from_slice(&24u32.to_be_bytes());
        idx += 4;
        block[idx..idx + 4].copy_from_slice(&0u32.to_be_bytes());
        idx += 4;
        // Data length
        block[idx..idx + 4].copy_from_slice(&(jpeg_payload.len() as u32).to_be_bytes());
        idx += 4;
        block[idx..idx + jpeg_payload.len()].copy_from_slice(&jpeg_payload);
        idx += jpeg_payload.len();

        let block_len = idx;
        let mut flac_data = [0u8; 256];
        flac_data[0..4].copy_from_slice(b"fLaC");
        // Block header: is_last = 1, type = 6
        flac_data[4] = 0x80 | 6;
        flac_data[5] = ((block_len >> 16) & 0xFF) as u8;
        flac_data[6] = ((block_len >> 8) & 0xFF) as u8;
        flac_data[7] = (block_len & 0xFF) as u8;
        flac_data[8..8 + block_len].copy_from_slice(&block[..block_len]);

        let mut out = [0u8; MAX_ART_BYTES];
        let n = extract_flac_picture(&flac_data[..8 + block_len], &mut out).expect("FLAC PICTURE should be extracted");
        assert_eq!(n, jpeg_payload.len());
        assert_eq!(&out[..n], &jpeg_payload);
    }

    #[test]
    fn test_corrupt_picture_magic_rejected() {
        let corrupt_payload = [0x01, 0x02, 0x03, 0x04];
        let mut apic_body = [0u8; 64];
        let mut idx = 0;
        apic_body[idx] = 0;
        idx += 1;
        let mime = b"image/jpeg\0";
        apic_body[idx..idx + mime.len()].copy_from_slice(mime);
        idx += mime.len();
        apic_body[idx] = 3;
        idx += 1;
        apic_body[idx] = 0;
        idx += 1;
        apic_body[idx..idx + corrupt_payload.len()].copy_from_slice(&corrupt_payload);
        idx += corrupt_payload.len();

        let frame_size = idx as u32;
        let mut tag = [0u8; 128];
        tag[0..3].copy_from_slice(b"ID3");
        tag[3] = 3;
        let total_tag_len = (10 + frame_size) as usize;
        tag[6] = ((total_tag_len >> 21) & 0x7F) as u8;
        tag[7] = ((total_tag_len >> 14) & 0x7F) as u8;
        tag[8] = ((total_tag_len >> 7) & 0x7F) as u8;
        tag[9] = (total_tag_len & 0x7F) as u8;

        tag[10..14].copy_from_slice(b"APIC");
        tag[14..18].copy_from_slice(&frame_size.to_be_bytes());
        tag[20..20 + idx].copy_from_slice(&apic_body[..idx]);

        let mut out = [0u8; MAX_ART_BYTES];
        let res = extract_id3v2_apic(&tag[..20 + idx], &mut out);
        assert_eq!(res, Err(ArtError::Corrupt));
    }

    #[test]
    fn test_absent_art_returns_no_art() {
        let mut out = [0u8; MAX_ART_BYTES];
        let flac_no_pic = [b'f', b'L', b'a', b'C', 0x80 | 0, 0, 0, 4, 0, 0, 0, 0];
        assert_eq!(extract_album_art(&flac_no_pic, &mut out), Err(ArtError::NoArt));
    }
}
