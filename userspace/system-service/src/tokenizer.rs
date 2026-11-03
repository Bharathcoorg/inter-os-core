// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Lightweight, zero-allocation tokenizer for Gemma vocabulary and byte-fallback encoding.
//!
//! Fully `#![no_std]` compliant with zero heap allocations.
//! Supports SentencePiece / Byte-Pair Encoding (BPE) subword lookup and byte-fallback
//! representation for arbitrary UTF-8 text.

pub const TOKEN_PAD: u32 = 0;
pub const TOKEN_EOS: u32 = 1;
pub const TOKEN_BOS: u32 = 2;
pub const TOKEN_UNK: u32 = 3;

/// Byte-fallback offset: byte values 0..=255 map to token IDs 4..=259.
pub const BYTE_FALLBACK_OFFSET: u32 = 4;
/// Number of byte-fallback tokens.
pub const BYTE_FALLBACK_COUNT: u32 = 256;

/// Common Gemma subword tokens for vocabulary matching (id >= 260).
pub struct SubwordToken {
    pub id: u32,
    pub text: &'static [u8],
}

pub static COMMON_SUBWORDS: &[SubwordToken] = &[
    SubwordToken { id: 260, text: b" " },
    SubwordToken { id: 261, text: b" the" },
    SubwordToken { id: 262, text: b" The" },
    SubwordToken { id: 263, text: b" of" },
    SubwordToken { id: 264, text: b" to" },
    SubwordToken { id: 265, text: b" and" },
    SubwordToken { id: 266, text: b" in" },
    SubwordToken { id: 267, text: b" is" },
    SubwordToken { id: 268, text: b" that" },
    SubwordToken { id: 269, text: b" for" },
    SubwordToken { id: 270, text: b" system" },
    SubwordToken { id: 271, text: b" System" },
    SubwordToken { id: 272, text: b" model" },
    SubwordToken { id: 273, text: b" query" },
    SubwordToken { id: 274, text: b" device" },
    SubwordToken { id: 275, text: b" battery" },
    SubwordToken { id: 276, text: b" status" },
    SubwordToken { id: 277, text: b" online" },
    SubwordToken { id: 278, text: b" offline" },
    SubwordToken { id: 279, text: b" memory" },
    SubwordToken { id: 280, text: b" storage" },
    SubwordToken { id: 281, text: b" kernel" },
    SubwordToken { id: 282, text: b" network" },
    SubwordToken { id: 283, text: b" display" },
    SubwordToken { id: 284, text: b" audio" },
    SubwordToken { id: 285, text: b" ok" },
    SubwordToken { id: 286, text: b" error" },
    SubwordToken { id: 287, text: b" warning" },
    SubwordToken { id: 288, text: b" ready" },
    SubwordToken { id: 289, text: b" active" },
];

/// Encodes an input text slice into a token sequence with zero heap allocation.
///
/// Greedily matches the longest common subword or falls back to byte tokens.
/// Returns the number of tokens written into `out_tokens`.
pub fn encode(text: &[u8], out_tokens: &mut [u32], add_bos: bool) -> usize {
    let mut num_tokens = 0usize;
    if add_bos && num_tokens < out_tokens.len() {
        out_tokens[num_tokens] = TOKEN_BOS;
        num_tokens += 1;
    }

    let mut i = 0usize;
    while i < text.len() && num_tokens < out_tokens.len() {
        let remaining = &text[i..];
        let mut matched_token: Option<(u32, usize)> = None;

        // Longest-prefix match across common subwords
        for subword in COMMON_SUBWORDS {
            if remaining.starts_with(subword.text) {
                if let Some((_, best_len)) = matched_token {
                    if subword.text.len() > best_len {
                        matched_token = Some((subword.id, subword.text.len()));
                    }
                } else {
                    matched_token = Some((subword.id, subword.text.len()));
                }
            }
        }

        if let Some((token_id, match_len)) = matched_token {
            out_tokens[num_tokens] = token_id;
            num_tokens += 1;
            i += match_len;
        } else {
            // Byte fallback: map byte text[i] to byte token
            let b = text[i];
            out_tokens[num_tokens] = BYTE_FALLBACK_OFFSET + (b as u32);
            num_tokens += 1;
            i += 1;
        }
    }

    num_tokens
}

/// Decodes a single token ID into UTF-8 bytes in `out_buf`.
///
/// Returns the number of bytes written, or 0 if token is special / invisible.
pub fn decode_token(token_id: u32, out_buf: &mut [u8]) -> usize {
    if token_id == TOKEN_PAD || token_id == TOKEN_EOS || token_id == TOKEN_BOS || token_id == TOKEN_UNK {
        return 0;
    }

    // Check byte fallback range
    if token_id >= BYTE_FALLBACK_OFFSET && token_id < BYTE_FALLBACK_OFFSET + BYTE_FALLBACK_COUNT {
        let b = (token_id - BYTE_FALLBACK_OFFSET) as u8;
        if !out_buf.is_empty() {
            out_buf[0] = b;
            return 1;
        }
        return 0;
    }

    // Check subword vocabulary
    for subword in COMMON_SUBWORDS {
        if subword.id == token_id {
            let len = subword.text.len().min(out_buf.len());
            out_buf[..len].copy_from_slice(&subword.text[..len]);
            return len;
        }
    }

    0
}

/// Decodes a sequence of tokens into UTF-8 text in `out_buf`.
/// Returns total bytes written.
pub fn decode_sequence(tokens: &[u32], out_buf: &mut [u8]) -> usize {
    let mut total = 0usize;
    for &tok in tokens {
        if total >= out_buf.len() {
            break;
        }
        let written = decode_token(tok, &mut out_buf[total..]);
        total += written;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_decode_roundtrip() {
        let text = b"The system status is ready";
        let mut tokens = [0u32; 32];
        let num_tok = encode(text, &mut tokens, false);
        assert!(num_tok > 0);

        let mut out_text = [0u8; 64];
        let decoded_len = decode_sequence(&tokens[..num_tok], &mut out_text);
        assert_eq!(&out_text[..decoded_len], text);
    }

    #[test]
    fn test_byte_fallback_roundtrip() {
        let text = b"xyz123!@#$";
        let mut tokens = [0u32; 32];
        let num_tok = encode(text, &mut tokens, false);
        assert_eq!(num_tok, text.len());

        let mut out_text = [0u8; 32];
        let decoded_len = decode_sequence(&tokens[..num_tok], &mut out_text);
        assert_eq!(&out_text[..decoded_len], text);
    }

    #[test]
    fn test_bos_token_generation() {
        let text = b"query";
        let mut tokens = [0u32; 16];
        let num_tok = encode(text, &mut tokens, true);
        assert_eq!(tokens[0], TOKEN_BOS);
        assert!(num_tok >= 2);
    }
}
