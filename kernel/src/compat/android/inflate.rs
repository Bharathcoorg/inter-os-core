// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded, allocation-free DEFLATE decoder for Android APK container extraction.
//!
//! Provides bare-metal `no_std` RFC 1951 raw DEFLATE stream decompression
//! for compressed APK members (`classes.dex`, `AndroidManifest.xml`, assets)
//! with zero external third-party dependencies.

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

/// Why a DEFLATE stream could not be decoded.
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

    /// Decodes one symbol using canonical code lengths.
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
        let mut source = *written - distance;
        while *written < out.len() {
            out[*written] = out[source];
            *written += 1;
            source += 1;
        }
        return Err(InflateError::OutputLimit);
    }
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
                    while written < out.len() {
                        out[written] = reader.bits(8)? as u8;
                        written += 1;
                    }
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

    if lengths[256] == 0 {
        return Err(InflateError::InvalidHuffman);
    }

    let mut literal_tree = Huffman::EMPTY;
    literal_tree.build(&lengths[..literal_count])?;
    let mut distance_tree = Huffman::EMPTY;
    distance_tree.build(&lengths[literal_count..total])?;
    Ok((literal_tree, distance_tree))
}

/// Decodes a raw DEFLATE stream into caller-provided slice.
pub fn inflate_raw(input: &[u8], out: &mut [u8]) -> Result<usize, InflateError> {
    let mut reader = BitReader::new(input);
    inflate_compressed(&mut reader, out)
}
