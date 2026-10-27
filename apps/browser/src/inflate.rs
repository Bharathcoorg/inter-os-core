//! Bounded, allocation-free DEFLATE, zlib and gzip decoders.
//!
//! WHY this exists: the gateway may hand the browser a body that the origin
//! compressed. The reader cannot parse markup until the bytes are the
//! origin's bytes, and the browser has no heap and no third-party crates, so
//! the decoder lives here.
//!
//! The decoder implements RFC 1951 (DEFLATE), RFC 1952 (gzip) and RFC 1950
//! (zlib). It is deliberately a bounded one-shot decoder: the caller names
//! the output buffer, and a stream that would write past it stops with
//! [`InflateError::OutputLimit`] instead of growing anything. Window
//! distances are checked against the bytes actually produced, so malformed
//! input cannot make the decoder read outside the output buffer.

/// The largest Huffman code length DEFLATE permits.
const MAX_BITS: usize = 15;

/// The number of literal/length symbols in a dynamic block.
const MAX_LIT_CODES: usize = 288;

/// The number of distance symbols a dynamic block can declare.
const MAX_DIST_CODES: usize = 32;

/// The order the code-length alphabet is transmitted in.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Length base values for symbols 257..=285.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59,
    67, 83, 99, 115, 131, 163, 195, 227, 258,
];

/// Extra bits for each length symbol.
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5,
    5, 5, 5, 0,
];

/// Distance base values for symbols 0..=29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513,
    769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

/// Extra bits for each distance symbol.
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10,
    11, 11, 12, 12, 13, 13,
];

/// Why a stream could not be decoded.
///
/// Every variant names the check that refused the input; the browser reports
/// the failure rather than showing a partial body as a complete one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The input ended in the middle of a symbol or a stored block.
    Truncated,
    /// BTYPE is the reserved value 3.
    InvalidBlockType,
    /// A stored block's LEN and NLEN do not complement each other.
    InvalidStoredLength,
    /// A dynamic block declared a code that cannot be decoded.
    InvalidHuffman,
    /// A literal/length symbol outside the alphabet.
    InvalidSymbol,
    /// A distance points before the start of the output.
    InvalidDistance,
    /// The decoded body is larger than the caller's buffer.
    OutputLimit,
    /// The gzip header is malformed or uses a reserved flag.
    BadGzipHeader,
    /// A compression method other than 8 (DEFLATE) was requested.
    UnsupportedMethod,
    /// The gzip CRC32 or zlib Adler-32 did not match the output.
    BadChecksum,
    /// The zlib stream names a preset dictionary the browser does not have.
    DictionaryRequired,
    /// A wrapper header (gzip/zlib) was shorter than its fixed fields.
    BadWrapper,
}

struct BitReader<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            bit: 0,
        }
    }

    /// Reads `count` bits least-significant bit first, as DEFLATE stores them.
    fn bits(&mut self, count: u8) -> Result<u32, InflateError> {
        let mut value = 0u32;
        for index in 0..count {
            if self.byte >= self.data.len() {
                return Err(InflateError::Truncated);
            }
            let bit = (self.data[self.byte] >> self.bit) & 1;
            value |= (bit as u32) << index;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
        Ok(value)
    }

    /// Skips to the next byte boundary.
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.byte += 1;
        }
    }

    /// The number of whole input bytes consumed so far.
    fn consumed(&self) -> usize {
        if self.bit == 0 {
            self.byte
        } else {
            self.byte + 1
        }
    }
}

#[derive(Clone, Copy)]
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: [u16; MAX_LIT_CODES],
}

impl Huffman {
    const EMPTY: Self = Self {
        counts: [0; MAX_BITS + 1],
        symbols: [0; MAX_LIT_CODES],
    };

    /// Builds the canonical code counts and symbol order (RFC 1951 3.2.2).
    fn build(&mut self, lengths: &[u8]) -> Result<(), InflateError> {
        self.counts = [0; MAX_BITS + 1];
        self.symbols = [0; MAX_LIT_CODES];
        if lengths.len() > self.symbols.len() {
            return Err(InflateError::InvalidHuffman);
        }
        for &length in lengths {
            if length as usize > MAX_BITS {
                return Err(InflateError::InvalidHuffman);
            }
            self.counts[length as usize] += 1;
        }
        // Zero-length codes are not symbols and must not take part in the
        // canonical ordering.
        self.counts[0] = 0;
        let mut offsets = [0u16; MAX_BITS + 1];
        for length in 1..=MAX_BITS {
            offsets[length] = offsets[length - 1] + self.counts[length - 1];
        }
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let slot = offsets[length as usize] as usize;
            if slot >= self.symbols.len() {
                return Err(InflateError::InvalidHuffman);
            }
            self.symbols[slot] = symbol as u16;
            offsets[length as usize] += 1;
        }
        Ok(())
    }

    /// Decodes one symbol. This is the canonical decode loop from puff.c:
    /// each bit extends the code and the counts say when a code of this
    /// length starts.
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, InflateError> {
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0u32;
        for length in 1..=MAX_BITS {
            code |= reader.bits(1)?;
            let count = self.counts[length] as u32;
            if code >= first && code - first < count {
                let slot = index + (code - first);
                if slot as usize >= self.symbols.len() {
                    return Err(InflateError::InvalidHuffman);
                }
                return Ok(self.symbols[slot as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::InvalidHuffman)
    }
}

/// Builds the fixed literal/length tree from RFC 1951 3.2.6.
fn fixed_literal_tree() -> Result<Huffman, InflateError> {
    let mut lengths = [0u8; MAX_LIT_CODES];
    for (symbol, slot) in lengths.iter_mut().enumerate() {
        *slot = match symbol {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let mut tree = Huffman::EMPTY;
    tree.build(&lengths)?;
    Ok(tree)
}

/// Builds the fixed distance tree: 32 five-bit codes.
fn fixed_distance_tree() -> Result<Huffman, InflateError> {
    let mut tree = Huffman::EMPTY;
    tree.build(&[5u8; MAX_DIST_CODES])?;
    Ok(tree)
}

fn copy_match(
    out: &mut [u8],
    written: &mut usize,
    distance: usize,
    length: usize,
) -> Result<(), InflateError> {
    if distance == 0 || distance > *written {
        return Err(InflateError::InvalidDistance);
    }
    if *written + length > out.len() {
        return Err(InflateError::OutputLimit);
    }
    // A match may overlap the bytes it is producing; copying one byte at a
    // time is what makes that well-defined and is bounded by `length`.
    let mut source = *written - distance;
    for _ in 0..length {
        out[*written] = out[source];
        *written += 1;
        source += 1;
    }
    Ok(())
}

fn inflate_compressed(
    reader: &mut BitReader<'_>,
    out: &mut [u8],
) -> Result<usize, InflateError> {
    let mut written = 0usize;
    let mut final_block = false;
    while !final_block {
        final_block = reader.bits(1)? != 0;
        let block_type = reader.bits(2)?;
        match block_type {
            0 => {
                reader.align();
                let len = reader.bits(16)? as usize;
                let nlen = reader.bits(16)? as usize;
                if len != (!nlen & 0xFFFF) {
                    return Err(InflateError::InvalidStoredLength);
                }
                if written + len > out.len() {
                    return Err(InflateError::OutputLimit);
                }
                for _ in 0..len {
                    out[written] = reader.bits(8)? as u8;
                    written += 1;
                }
            }
            1 | 2 => {
                let literal_tree;
                let distance_tree;
                if block_type == 1 {
                    literal_tree = fixed_literal_tree()?;
                    distance_tree = fixed_distance_tree()?;
                } else {
                    let (literal, distance) = dynamic_trees(reader)?;
                    literal_tree = literal;
                    distance_tree = distance;
                }
                loop {
                    let symbol = literal_tree.decode(reader)? as usize;
                    match symbol {
                        0..=255 => {
                            if written >= out.len() {
                                return Err(InflateError::OutputLimit);
                            }
                            out[written] = symbol as u8;
                            written += 1;
                        }
                        256 => break,
                        257..=285 => {
                            let slot = symbol - 257;
                            let length = LENGTH_BASE[slot] as usize
                                + reader.bits(LENGTH_EXTRA[slot])? as usize;
                            let distance_symbol = distance_tree.decode(reader)? as usize;
                            if distance_symbol > 29 {
                                return Err(InflateError::InvalidDistance);
                            }
                            let distance = DIST_BASE[distance_symbol] as usize
                                + reader.bits(DIST_EXTRA[distance_symbol])? as usize;
                            copy_match(out, &mut written, distance, length)?;
                        }
                        _ => return Err(InflateError::InvalidSymbol),
                    }
                }
            }
            _ => return Err(InflateError::InvalidBlockType),
        }
    }
    Ok(written)
}

/// Reads a dynamic block's two Huffman trees.
fn dynamic_trees(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman), InflateError> {
    let literal_count = reader.bits(5)? as usize + 257;
    let distance_count = reader.bits(5)? as usize + 1;
    let code_length_count = reader.bits(4)? as usize + 4;
    if literal_count > 286 || distance_count > MAX_DIST_CODES || code_length_count > 19 {
        return Err(InflateError::InvalidHuffman);
    }

    let mut code_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_lengths[position] = reader.bits(3)? as u8;
    }
    let mut code_tree = Huffman::EMPTY;
    code_tree.build(&code_lengths)?;

    let mut lengths = [0u8; MAX_LIT_CODES + MAX_DIST_CODES];
    let total = literal_count + distance_count;
    if total > lengths.len() {
        return Err(InflateError::InvalidHuffman);
    }
    let mut index = 0usize;
    while index < total {
        let symbol = code_tree.decode(reader)?;
        match symbol {
            0..=15 => {
                lengths[index] = symbol as u8;
                index += 1;
            }
            16 => {
                if index == 0 {
                    return Err(InflateError::InvalidHuffman);
                }
                let repeat = 3 + reader.bits(2)? as usize;
                let previous = lengths[index - 1];
                if index + repeat > total {
                    return Err(InflateError::InvalidHuffman);
                }
                for _ in 0..repeat {
                    lengths[index] = previous;
                    index += 1;
                }
            }
            17 => {
                let repeat = 3 + reader.bits(3)? as usize;
                if index + repeat > total {
                    return Err(InflateError::InvalidHuffman);
                }
                index += repeat;
            }
            18 => {
                let repeat = 11 + reader.bits(7)? as usize;
                if index + repeat > total {
                    return Err(InflateError::InvalidHuffman);
                }
                index += repeat;
            }
            _ => return Err(InflateError::InvalidHuffman),
        }
    }

    // The end-of-block symbol must exist or the block would never stop.
    if lengths[256] == 0 {
        return Err(InflateError::InvalidHuffman);
    }

    let mut literal_tree = Huffman::EMPTY;
    literal_tree.build(&lengths[..literal_count])?;
    let mut distance_tree = Huffman::EMPTY;
    distance_tree.build(&lengths[literal_count..total])?;
    Ok((literal_tree, distance_tree))
}

/// Decodes a raw DEFLATE stream and returns the number of output bytes.
pub fn inflate_raw(input: &[u8], out: &mut [u8]) -> Result<usize, InflateError> {
    let mut reader = BitReader::new(input);
    inflate_compressed(&mut reader, out)
}

/// Decodes a raw DEFLATE stream and reports how many input bytes it consumed.
///
/// WHY the consumed count: gzip's CRC and length trailer starts at the next
/// byte after the final block, so the wrapper has to know where DEFLATE
/// stopped.
pub fn inflate_raw_consumed(input: &[u8], out: &mut [u8]) -> Result<(usize, usize), InflateError> {
    let mut reader = BitReader::new(input);
    let written = inflate_compressed(&mut reader, out)?;
    Ok((written, reader.consumed()))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn read_be32(data: &[u8], at: usize) -> Option<u32> {
    if at + 4 > data.len() {
        return None;
    }
    Some(
        (data[at] as u32) << 24
            | (data[at + 1] as u32) << 16
            | (data[at + 2] as u32) << 8
            | data[at + 3] as u32,
    )
}

fn read_le32(data: &[u8], at: usize) -> Option<u32> {
    if at + 4 > data.len() {
        return None;
    }
    Some(
        data[at] as u32
            | (data[at + 1] as u32) << 8
            | (data[at + 2] as u32) << 16
            | (data[at + 3] as u32) << 24,
    )
}

/// Decodes a gzip (RFC 1952) member and verifies its CRC32 and ISIZE.
pub fn gunzip(input: &[u8], out: &mut [u8]) -> Result<usize, InflateError> {
    if input.len() < 18 {
        return Err(InflateError::BadWrapper);
    }
    if input[0] != 0x1F || input[1] != 0x8B {
        return Err(InflateError::BadGzipHeader);
    }
    if input[2] != 8 {
        return Err(InflateError::UnsupportedMethod);
    }
    let flags = input[3];
    if flags & 0xE0 != 0 {
        return Err(InflateError::BadGzipHeader);
    }
    let mut position = 10usize;
    if flags & 0x04 != 0 {
        // FEXTRA: a two-byte length followed by that many bytes.
        if position + 2 > input.len() {
            return Err(InflateError::Truncated);
        }
        let extra = input[position] as usize | (input[position + 1] as usize) << 8;
        position += 2;
        if position + extra > input.len() {
            return Err(InflateError::Truncated);
        }
        position += extra;
    }
    for flag in [0x08u8, 0x10u8] {
        // FNAME and FCOMMENT are zero-terminated strings.
        if flags & flag != 0 {
            while position < input.len() && input[position] != 0 {
                position += 1;
            }
            if position >= input.len() {
                return Err(InflateError::Truncated);
            }
            position += 1;
        }
    }
    if flags & 0x02 != 0 {
        // FHCRC: the two header CRC bytes are not verified; they are skipped.
        if position + 2 > input.len() {
            return Err(InflateError::Truncated);
        }
        position += 2;
    }

    let (written, consumed) = inflate_raw_consumed(&input[position..], out)?;
    let trailer = position + consumed;
    if trailer + 8 > input.len() {
        return Err(InflateError::Truncated);
    }
    let expected_crc = read_le32(input, trailer).ok_or(InflateError::Truncated)?;
    let expected_size = read_le32(input, trailer + 4).ok_or(InflateError::Truncated)?;
    if expected_crc != crc32(&out[..written]) {
        return Err(InflateError::BadChecksum);
    }
    if expected_size != written as u32 {
        return Err(InflateError::BadChecksum);
    }
    Ok(written)
}

/// Decodes a zlib (RFC 1950) stream and verifies its Adler-32.
pub fn inflate_zlib(input: &[u8], out: &mut [u8]) -> Result<usize, InflateError> {
    if input.len() < 6 {
        return Err(InflateError::BadWrapper);
    }
    let cmf = input[0];
    let flg = input[1];
    if cmf & 0x0F != 8 {
        return Err(InflateError::UnsupportedMethod);
    }
    if cmf >> 4 > 7 {
        return Err(InflateError::BadWrapper);
    }
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(InflateError::BadWrapper);
    }
    if flg & 0x20 != 0 {
        return Err(InflateError::DictionaryRequired);
    }
    let (written, consumed) = inflate_raw_consumed(&input[2..], out)?;
    let trailer = 2 + consumed;
    if trailer + 4 > input.len() {
        return Err(InflateError::Truncated);
    }
    // zlib stores Adler-32 big-endian, unlike gzip's little-endian trailer.
    let expected = read_be32(input, trailer).ok_or(InflateError::Truncated)?;
    if expected != adler32(&out[..written]) {
        return Err(InflateError::BadChecksum);
    }
    Ok(written)
}

/// Decodes a body according to its `Content-Encoding` value.
///
/// `deflate` is ambiguous in the wild: RFC 2616 named zlib, but many servers
/// send raw DEFLATE. The zlib header is tried first and a stream that does
/// not carry one is decoded as raw.
pub fn decode_content_encoding(
    encoding: &[u8],
    input: &[u8],
    out: &mut [u8],
) -> Result<usize, InflateError> {
    let raw_name = encoding.split(|&b| b == b';').next().unwrap_or(encoding);
    let name_buf = normalized_encoding(raw_name);
    let name_len = name_buf
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name_buf.len());
    let name = &name_buf[..name_len];
    if name.is_empty() || name == b"identity" {
        let len = input.len().min(out.len());
        out[..len].copy_from_slice(&input[..len]);
        if len < input.len() {
            return Err(InflateError::OutputLimit);
        }
        return Ok(len);
    }
    if name == b"gzip" || name == b"x-gzip" {
        return gunzip(input, out);
    }
    if name == b"deflate" {
        if input.len() >= 2
            && input[0] & 0x0F == 8
            && input[0] >> 4 <= 7
            && (((input[0] as u16) << 8 | input[1] as u16) % 31 == 0)
        {
            return inflate_zlib(input, out);
        }
        return inflate_raw(input, out);
    }
    Err(InflateError::UnsupportedMethod)
}

/// Lower-cases a content-coding token into a fixed buffer.
///
/// WHY fixed: the encoding name is a single token, and a name longer than
/// this is not one of the codings the decoder supports.
fn normalized_encoding(raw: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut len = 0;
    for &byte in raw {
        if (byte as char).is_ascii_whitespace() {
            continue;
        }
        if len == out.len() {
            break;
        }
        out[len] = byte.to_ascii_lowercase();
        len += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> &[u8] {
        bytes
    }

    #[test]
    fn a_stored_block_copies_its_bytes() {
        let raw = hex(b"\x01\x05\x00\xfa\xffhello");
        let mut out = [0u8; 16];
        assert_eq!(inflate_raw(raw, &mut out), Ok(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn a_fixed_huffman_block_decompresses() {
        let raw = hex(b"\xcb\x48\xcd\xc9\xc9\x07\x00");
        let mut out = [0u8; 16];
        assert_eq!(inflate_raw(raw, &mut out), Ok(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn a_dynamic_huffman_block_decompresses() {
        // 100 bytes drawn from a skewed alphabet; Python's zlib chose a
        // dynamic block (BTYPE = 2) for this stream.
        let raw = hex(b"\x35\x8b\x81\x09\xc0\x00\x0c\xc2\x6e\x8d\xae\xff\xbf\xb0\x38\x18\x68\x91\x54\x21\x07\x17\x7d\x05\x66\x22\x6c\xfb\x18\xe4\xa4\xa9\x1a\x1e\xc9\x57\x36\xd9\xf5\xb1\xd1\xd0\xba\x7f\xf4\x38\x77\x31\xe5\x05");
        let expected = hex(b"\x61\x61\x62\x65\x61\x61\x65\x62\x61\x61\x65\x65\x63\x61\x61\x61\x63\x61\x61\x61\x61\x62\x61\x62\x65\x63\x63\x63\x64\x61\x62\x61\x61\x61\x65\x61\x62\x63\x62\x63\x63\x62\x63\x61\x62\x61\x62\x61\x62\x61\x61\x62\x62\x61\x61\x65\x65\x61\x62\x61\x63\x61\x61\x62\x63\x63\x61\x61\x61\x63\x62\x62\x61\x61\x62\x61\x61\x61\x61\x61\x63\x62\x62\x61\x61\x62\x62\x62\x61\x63\x63\x64\x63\x61\x62\x63\x61\x62\x63\x62");
        let mut out = [0u8; 128];
        assert_eq!(inflate_raw(raw, &mut out), Ok(100));
        assert_eq!(&out[..100], expected);
    }

    #[test]
    fn a_gzip_member_is_checked_against_its_crc_and_size() {
        let gz = hex(b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xcb\x48\xcd\xc9\xc9\x07\x00\x86\xa6\x10\x36\x05\x00\x00\x00");
        let mut out = [0u8; 16];
        assert_eq!(gunzip(gz, &mut out), Ok(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn a_gzip_crc_mismatch_is_reported_not_ignored() {
        let mut gz = [0u8; 25];
        gz.copy_from_slice(b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xcb\x48\xcd\xc9\xc9\x07\x00\x86\xa6\x10\x36\x05\x00\x00\x00");
        gz[17] ^= 0xFF; // first CRC byte
        let mut out = [0u8; 16];
        assert_eq!(gunzip(&gz, &mut out), Err(InflateError::BadChecksum));
    }

    #[test]
    fn a_zlib_stream_is_checked_against_adler32() {
        let z = hex(b"\x78\xda\xcb\x48\xcd\xc9\xc9\x07\x00\x06\x2c\x02\x15");
        let mut out = [0u8; 16];
        assert_eq!(inflate_zlib(z, &mut out), Ok(5));
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn malformed_and_truncated_streams_are_refused() {
        let mut out = [0u8; 64];
        assert_eq!(inflate_raw(b"", &mut out), Err(InflateError::Truncated));
        assert_eq!(
            inflate_raw(b"\x07", &mut out),
            Err(InflateError::InvalidBlockType)
        );
        // Stored block whose NLEN is not LEN's complement.
        assert_eq!(
            inflate_raw(b"\x01\x05\x00\x00\x00hello", &mut out),
            Err(InflateError::InvalidStoredLength)
        );
        // A stored block that promises more bytes than the input holds.
        assert_eq!(
            inflate_raw(b"\x01\x05\x00\xfa\xffhi", &mut out),
            Err(InflateError::Truncated)
        );
    }

    #[test]
    fn the_output_buffer_is_a_hard_limit() {
        let raw = hex(b"\xcb\x48\xcd\xc9\xc9\x07\x00");
        let mut small = [0u8; 4];
        assert_eq!(
            inflate_raw(raw, &mut small),
            Err(InflateError::OutputLimit)
        );
        let gz = hex(b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xcb\x48\xcd\xc9\xc9\x07\x00\x86\xa6\x10\x36\x05\x00\x00\x00");
        let mut small = [0u8; 2];
        assert_eq!(gunzip(gz, &mut small), Err(InflateError::OutputLimit));
    }

    fn push_bit(bytes: &mut [u8; 64], position: &mut usize, bit: u8) {
        if bit != 0 {
            bytes[*position / 8] |= 1 << (*position % 8);
        }
        *position += 1;
    }

    fn push_code(bytes: &mut [u8; 64], position: &mut usize, code: u32, length: u8) {
        for shift in (0..length).rev() {
            push_bit(bytes, position, ((code >> shift) & 1) as u8);
        }
    }

    #[test]
    fn a_distance_before_the_output_is_refused() {
        // Fixed block: BFINAL=1, BTYPE=01, literal 'a' (code 0x91), then
        // length 3 and distance 2 while only one byte has been produced.
        let mut bits = [0u8; 64];
        let mut position = 0usize;
        push_bit(&mut bits, &mut position, 1);
        push_bit(&mut bits, &mut position, 1);
        push_bit(&mut bits, &mut position, 0);
        push_code(&mut bits, &mut position, 0x91, 8); // literal 'a'
        push_code(&mut bits, &mut position, 1, 7); // length symbol 257
        push_code(&mut bits, &mut position, 1, 5); // distance symbol 1
        let raw = &bits[..(position + 7) / 8];
        let mut out = [0u8; 16];
        assert_eq!(inflate_raw(raw, &mut out), Err(InflateError::InvalidDistance));
    }

    #[test]
    fn an_overlapping_match_repeats_the_byte_it_copies() {
        // Fixed block: literal 'a', length 258 (symbol 285, code 0xC5),
        // distance 1 (symbol 0), then end-of-block.
        let mut bits = [0u8; 64];
        let mut position = 0usize;
        push_bit(&mut bits, &mut position, 1);
        push_bit(&mut bits, &mut position, 1);
        push_bit(&mut bits, &mut position, 0);
        push_code(&mut bits, &mut position, 0x91, 8);
        push_code(&mut bits, &mut position, 0xC5, 8);
        push_code(&mut bits, &mut position, 0, 5);
        push_code(&mut bits, &mut position, 0, 7); // end of block
        let raw = &bits[..(position + 7) / 8];
        let mut out = [0u8; 512];
        assert_eq!(inflate_raw(raw, &mut out), Ok(259));
        assert!(out[..259].iter().all(|&byte| byte == b'a'));
    }

    #[test]
    fn identity_and_unknown_encodings_are_reported() {
        let input = b"plain";
        let mut out = [0u8; 8];
        assert_eq!(
            decode_content_encoding(b"identity", input, &mut out),
            Ok(5)
        );
        assert_eq!(&out[..5], b"plain");
        assert_eq!(
            decode_content_encoding(b"br", input, &mut out),
            Err(InflateError::UnsupportedMethod)
        );
    }
}
