//! WAV container parsing and conversion to the controller's native PCM.
//!
//! WHAT this is: a reader for the uncompressed WAV files the device can hold,
//! plus a converter that turns their samples into the one format the AC'97
//! controller accepts (48 kHz, stereo, signed 16-bit little-endian).
//!
//! WHY the converter resamples instead of refusing other rates: a WAV is a
//! file the person put on the device, and refusing a 44.1 kHz file would make
//! the player unable to open the most common file it is likely to meet. The
//! resampler is linear, which is the quality this CPU can pay for in a media
//! player; it is not a claim of high-fidelity conversion.
//!
//! WHY only integer PCM is accepted: a float WAV's samples are IEEE floats,
//! and playing them as if they were integers would produce noise rather than
//! music. A format this module cannot decode is refused, never guessed at.

/// The rate the AC'97 controller is programmed for.
pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;

/// The most source frames one conversion step advances over.
const Q32_ONE: u64 = 1 << 32;

/// What a WAV header says about the samples that follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    /// Samples per second.
    pub sample_rate: u32,
    /// One or two channels.
    pub channels: u16,
    /// Bits per sample: 8, 16, 24 or 32.
    pub bits_per_sample: u16,
    /// Byte offset of the first sample in the file.
    pub data_offset: u64,
    /// Byte length the header claims for the sample data.
    pub data_len: u64,
}

impl WavInfo {
    /// Bytes one whole frame occupies.
    pub fn frame_bytes(&self) -> usize {
        self.channels as usize * (self.bits_per_sample as usize / 8)
    }

    /// Whole frames the file claims to hold.
    pub fn frame_count(&self) -> u64 {
        let frame = self.frame_bytes() as u64;
        if frame == 0 {
            return 0;
        }
        self.data_len / frame
    }

    /// The claimed length of the sound, in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.frame_count() as u128 * 1000 / self.sample_rate as u128) as u64
    }
}

/// Why a WAV could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// The file does not start with `RIFF`.
    NotRiff,
    /// The RIFF form is not `WAVE`.
    NotWave,
    /// The bytes end before the header is complete.
    Truncated,
    /// The samples are in a format this converter does not decode.
    UnsupportedFormat,
}

/// Reads the WAV header from the bytes at the start of the file.
///
/// Returns the description and the number of bytes the header occupies, which
/// is also the offset of the first sample. The caller may pass more than the
/// header; the extra bytes are ignored and the returned offset is where the
/// samples start.
pub fn parse_wav(head: &[u8]) -> Result<(WavInfo, usize), WavError> {
    if head.len() < 12 {
        return Err(WavError::Truncated);
    }
    if &head[0..4] != b"RIFF" {
        return Err(WavError::NotRiff);
    }
    if &head[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let mut format: Option<(u32, u16, u16)> = None;
    let mut at = 12usize;
    loop {
        if head.len() < at + 8 {
            return Err(WavError::Truncated);
        }
        let id = &head[at..at + 4];
        let size = u32::from_le_bytes([head[at + 4], head[at + 5], head[at + 6], head[at + 7]])
            as usize;
        let body = at + 8;
        if id == b"fmt " {
            if head.len() < body + 16 {
                return Err(WavError::Truncated);
            }
            let tag = u16::from_le_bytes([head[body], head[body + 1]]);
            let channels = u16::from_le_bytes([head[body + 2], head[body + 3]]);
            let rate = u32::from_le_bytes([
                head[body + 4],
                head[body + 5],
                head[body + 6],
                head[body + 7],
            ]);
            let bits = u16::from_le_bytes([head[body + 14], head[body + 15]]);
            if tag != 1 {
                return Err(WavError::UnsupportedFormat);
            }
            if !(1..=2).contains(&channels) {
                return Err(WavError::UnsupportedFormat);
            }
            if !matches!(bits, 8 | 16 | 24 | 32) {
                return Err(WavError::UnsupportedFormat);
            }
            if !(8_000..=192_000).contains(&rate) {
                return Err(WavError::UnsupportedFormat);
            }
            format = Some((rate, channels, bits));
        } else if id == b"data" {
            let Some((sample_rate, channels, bits_per_sample)) = format else {
                return Err(WavError::Truncated);
            };
            return Ok((
                WavInfo {
                    sample_rate,
                    channels,
                    bits_per_sample,
                    data_offset: body as u64,
                    data_len: size as u64,
                },
                body,
            ));
        }
        // A chunk with an odd length is followed by a pad byte, which is not
        // part of the chunk.
        let advance = 8usize
            .checked_add(size)
            .and_then(|n| n.checked_add(size & 1))
            .ok_or(WavError::Truncated)?;
        at = at.checked_add(advance).ok_or(WavError::Truncated)?;
    }
}

/// Converts source samples into the controller's native PCM.
///
/// The converter holds the interpolation state across blocks, so a caller may
/// hand it the file in pieces. It answers in whole source frames: bytes past
/// the last whole frame belong to the next block and the caller carries them.
#[derive(Clone, Copy, Debug)]
pub struct PcmConverter {
    info: WavInfo,
    /// Source frames per output frame, in Q32.32.
    step: u64,
    /// Position in the current block, in Q32.32 frames from the carried frame.
    phase: u64,
    /// The last frame of the previous block, extended to 16-bit values.
    carry: [i32; 2],
    primed: bool,
    volume: i32,
}

impl PcmConverter {
    /// Builds a converter for a parsed header.
    pub fn new(info: WavInfo) -> Result<Self, WavError> {
        if info.frame_bytes() == 0 || !(1..=2).contains(&info.channels) {
            return Err(WavError::UnsupportedFormat);
        }
        let step = if info.sample_rate == OUTPUT_SAMPLE_RATE {
            Q32_ONE
        } else {
            ((info.sample_rate as u64) << 32) / OUTPUT_SAMPLE_RATE as u64
        };
        Ok(Self {
            info,
            step,
            phase: 0,
            carry: [0; 2],
            primed: false,
            volume: 100,
        })
    }

    /// The header this converter was built for.
    pub fn info(&self) -> WavInfo {
        self.info
    }

    /// Sets the software volume, clamped to 0..=100.
    ///
    /// WHY the scaling is here: the controller has no volume register this
    /// platform programs, so a volume the player reports must be applied to
    /// the samples it hands over. A control that changed a number without
    /// changing the output would be a lie.
    pub fn set_volume_percent(&mut self, percent: u8) {
        self.volume = percent.min(100) as i32;
    }

    /// Converts as much of `src` as `out` can hold.
    ///
    /// Returns `(source bytes consumed, output bytes written)`. Both are whole
    /// frames, except that the source side stops before a split frame.
    pub fn convert(&mut self, src: &[u8], out: &mut [u8]) -> (usize, usize) {
        let frame_bytes = self.info.frame_bytes();
        let out_len = out.len() & !3;
        if frame_bytes == 0 || src.len() < frame_bytes || out_len < 4 {
            return (0, 0);
        }
        let frames = src.len() / frame_bytes;
        if self.info.sample_rate == OUTPUT_SAMPLE_RATE {
            let room = out_len / 4;
            let count = frames.min(room);
            for index in 0..count {
                let [left, right] = self.frame_at(src, index);
                self.write_frame(out, index * 4, left, right);
            }
            return (count * frame_bytes, count * 4);
        }

        if !self.primed {
            self.carry = self.frame_at(src, 0);
            self.primed = true;
        }
        // The sequence the interpolation walks is the carried frame followed
        // by this block's frames. Interpolating needs the frame after the
        // current position, so the final source frame is carried into the
        // next block instead of being emitted without a right neighbour.
        let sequence_len = frames + 1;
        let mut written = 0usize;
        let mut pos = self.phase;
        loop {
            if written + 4 > out_len {
                break;
            }
            let index = (pos >> 32) as usize;
            if index + 1 >= sequence_len {
                break;
            }
            let before = if index == 0 {
                self.carry
            } else {
                self.frame_at(src, index - 1)
            };
            let after = self.frame_at(src, index);
            let fraction = (pos & 0xFFFF_FFFF) as i64;
            let left = before[0] as i64 + (((after[0] - before[0]) as i64 * fraction) >> 32);
            let right = before[1] as i64 + (((after[1] - before[1]) as i64 * fraction) >> 32);
            self.write_frame(out, written, left as i32, right as i32);
            written += 4;
            pos = pos.saturating_add(self.step);
        }
        // The next block starts at the frame under the stopped position; the
        // frames before it are behind us and are the bytes this call consumed.
        let stopped_at = ((pos >> 32) as usize).min(frames);
        self.carry = if stopped_at == 0 {
            self.carry
        } else {
            self.frame_at(src, (stopped_at - 1).min(frames - 1))
        };
        self.phase = pos.saturating_sub((stopped_at as u64) << 32);
        (stopped_at.min(frames) * frame_bytes, written)
    }

    fn write_frame(&self, out: &mut [u8], at: usize, left: i32, right: i32) {
        let left = self.scale(left);
        let right = self.scale(right);
        out[at..at + 2].copy_from_slice(&left.to_le_bytes());
        out[at + 2..at + 4].copy_from_slice(&right.to_le_bytes());
    }

    fn scale(&self, sample: i32) -> i16 {
        let scaled = sample as i64 * self.volume as i64 / 100;
        scaled.clamp(i16::MIN as i64, i16::MAX as i64) as i16
    }

    /// Decodes one whole frame into signed 16-bit values.
    fn frame_at(&self, src: &[u8], index: usize) -> [i32; 2] {
        let frame_bytes = self.info.frame_bytes();
        let at = index * frame_bytes;
        let bytes = &src[at..at + frame_bytes];
        let first = decode_sample(bytes, self.info.bits_per_sample);
        if self.info.channels == 1 {
            [first, first]
        } else {
            let second = decode_sample(
                &bytes[self.info.bits_per_sample as usize / 8..],
                self.info.bits_per_sample,
            );
            [first, second]
        }
    }
}

fn decode_sample(bytes: &[u8], bits: u16) -> i32 {
    match bits {
        // 8-bit PCM is unsigned, centred on 128.
        8 => (bytes[0] as i32 - 128) << 8,
        16 => i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
        24 => {
            let raw = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
            // Sign-extend the 24-bit value, then keep the top 16 bits.
            ((raw << 8) >> 16) as i32
        }
        32 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> 16,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_off_the_end_is_not_read() {
        // A frame the header claims but the file does not hold is caught by
        // the caller shortening the slice; this pins the decoder's indexing.
        let info = WavInfo {
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 16,
            data_offset: 44,
            data_len: 4,
        };
        let converter = PcmConverter::new(info).unwrap();
        assert_eq!(converter.frame_at(&[0x10, 0x00, 0x20, 0x00], 1), [0x0020, 0x0020]);
    }
}
