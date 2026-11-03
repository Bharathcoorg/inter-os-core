// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The platform's image policy: the numbers every picture decoder is held to.
//!
//! WHY the policy is its own file: the numbers have to be the *same* numbers
//! for the raster decoders and for the vector reader, and the vector reader
//! is compiled on its own in the fixture suite (`tests/svg_fixtures.rs`
//! includes `src/image/svg.rs` directly). A file of constants and two checks
//! is the one shape that both of those can reach without either of them
//! reaching into the other.

/// The largest width or height, in pixels, that the platform will decode.
///
/// WHY a limit exists: every picture's dimensions come out of an untrusted
/// header, and a ten-byte file can name a picture of any size at all. The
/// caller's buffer already refuses anything that does not fit, but the
/// decoder's own fixed scratch (a row buffer pair) has to be sized for
/// *some* maximum, so the refusal is explicit and comes before any decoding
/// work: 4096 is wider than any surface this device paints, and it makes
/// every scratch size below a constant the linker can check.
pub const MAX_DECODE_DIMENSION: u32 = 4096;

/// The largest number of pixels one decoded picture may have.
///
/// WHY a pixel budget as well as a side limit: two sides of 4096 are
/// sixteen and a half million pixels and sixty-four megabytes of RGBA, which
/// no surface this device paints could hold and no caller should be asked to
/// size a buffer for. The budget is checked against the *declared* size, so
/// a hostile length field is refused before a buffer is sized from it.
pub const MAX_DECODE_PIXELS: usize = 8 * 1024 * 1024;

/// The largest encoded picture the platform will look at, in bytes.
///
/// WHY an encoded limit as well as a decoded one: the decoded budget bounds
/// the pixels a picture may have, not the work of reading it. A file that
/// claims to be one thing and carries four megabytes of padding, or a
/// vector document with a hundred thousand elements in it, is refused before
/// any of it is walked.
pub const MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;

/// Whether a declared size fits inside the decode budget.
///
/// WHY this is one function: every decoder's header is checked with it, so
/// there is exactly one answer to "is this picture too big for the
/// platform", and it is the same answer for every format. A decoder that
/// forgets the check is a decoder that lets a file choose its own memory.
pub fn check_decode_size(width: u32, height: u32) -> Result<(), ()> {
    if width == 0 || height == 0 {
        return Err(());
    }
    if width > MAX_DECODE_DIMENSION || height > MAX_DECODE_DIMENSION {
        return Err(());
    }
    let pixels = (width as usize).checked_mul(height as usize).ok_or(())?;
    if pixels > MAX_DECODE_PIXELS {
        return Err(());
    }
    Ok(())
}

/// Whether an encoded body is inside the encoded budget.
///
/// WHY this is checked before anything is parsed: the budget is about the
/// work of walking a body, and a body over the limit is not worth walking
/// to find out what it claims to be.
pub fn check_encoded_size(bytes: usize) -> Result<(), ()> {
    if bytes > MAX_ENCODED_BYTES {
        return Err(());
    }
    Ok(())
}
