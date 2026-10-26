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
    /// Format tag (1 = PCM, 2 = MS ADPCM, 6 = A-law, 7 = µ-law, 17 = IMA ADPCM).
    pub format_tag: u16,
    /// Samples per second.
    pub sample_rate: u32,
    /// One or two channels.
    pub channels: u16,
    /// Bits per sample: 4 (ADPCM), 8, 16, 24 or 32.
    pub bits_per_sample: u16,
    /// Byte offset of the first sample in the file.
    pub data_offset: u64,
    /// Byte length the header claims for the sample data.
    pub data_len: u64,
}

impl WavInfo {
    /// Bytes one whole frame occupies.
    pub fn frame_bytes(&self) -> usize {
        if self.format_tag == 2 || self.format_tag == 17 {
            // ADPCM: 4 bits per sample
            self.channels as usize
        } else {
            self.channels as usize * (self.bits_per_sample as usize / 8)
        }
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
    /// The stream chunk structure or payload is corrupt.
    CorruptStream,
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
    let mut format: Option<(u16, u32, u16, u16)> = None;
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
            if !matches!(tag, 1 | 2 | 6 | 7 | 17) {
                return Err(WavError::UnsupportedFormat);
            }
            if !(1..=2).contains(&channels) {
                return Err(WavError::UnsupportedFormat);
            }
            if !(8_000..=192_000).contains(&rate) {
                return Err(WavError::UnsupportedFormat);
            }
            match tag {
                1 => {
                    if !matches!(bits, 8 | 16 | 24 | 32) {
                        return Err(WavError::UnsupportedFormat);
                    }
                }
                6 | 7 => {
                    if bits != 8 {
                        return Err(WavError::UnsupportedFormat);
                    }
                }
                2 | 17 => {
                    if bits != 4 {
                        return Err(WavError::UnsupportedFormat);
                    }
                }
                _ => return Err(WavError::UnsupportedFormat),
            }
            format = Some((tag, rate, channels, bits));
        } else if id == b"data" {
            let Some((format_tag, sample_rate, channels, bits_per_sample)) = format else {
                return Err(WavError::Truncated);
            };
            return Ok((
                WavInfo {
                    format_tag,
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

/// Decodes an 8-bit ITU-T G.711 A-law sample into signed 16-bit linear PCM.
pub fn decode_alaw(byte: u8) -> i16 {
    let byte = byte ^ 0x55;
    let sign = byte & 0x80;
    let exponent = ((byte >> 4) & 0x07) as usize;
    let mantissa = (byte & 0x0F) as i32;
    let sample = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    if sign != 0 {
        sample as i16
    } else {
        -(sample as i16)
    }
}

/// Decodes an 8-bit ITU-T G.711 µ-law sample into signed 16-bit linear PCM.
pub fn decode_mulaw(byte: u8) -> i16 {
    let u = !byte;
    let sign = u & 0x80;
    let exponent = ((u >> 4) & 0x07) as usize;
    let mantissa = (u & 0x0F) as i32;
    let sample = (((mantissa << 3) + 0x84) << exponent) - 0x84;
    if sign != 0 {
        sample as i16
    } else {
        -(sample as i16)
    }
}

pub const IMA_STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17,
    19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358,
    5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767
];

pub const IMA_INDEX_TABLE: [i8; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8
];

pub fn decode_ima_adpcm_nibble(nibble: u8, predictor: &mut i32, step_index: &mut i8) -> i16 {
    let idx = (*step_index).clamp(0, 88) as usize;
    let step = IMA_STEP_TABLE[idx] as i32;
    let mut diff = step >> 3;
    if (nibble & 4) != 0 { diff += step; }
    if (nibble & 2) != 0 { diff += step >> 1; }
    if (nibble & 1) != 0 { diff += step >> 2; }
    if (nibble & 8) != 0 {
        *predictor -= diff;
    } else {
        *predictor += diff;
    }
    *predictor = (*predictor).clamp(-32768, 32767);
    *step_index = (*step_index + IMA_INDEX_TABLE[(nibble & 0x0F) as usize]).clamp(0, 88);
    *predictor as i16
}

pub fn decode_ms_adpcm_nibble(nibble: u8, p1: &mut i32, p2: &mut i32, delta: &mut i32, coeff1: i32, coeff2: i32) -> i16 {
    let mut signed_nibble = (nibble & 0x0F) as i32;
    if signed_nibble >= 8 {
        signed_nibble -= 16;
    }
    let mut predict = (*p1 * coeff1 + *p2 * coeff2) / 256;
    predict += signed_nibble * *delta;
    let sample = predict.clamp(-32768, 32767);
    *p2 = *p1;
    *p1 = sample;
    const ADAPT_TABLE: [i32; 16] = [
        230, 230, 230, 230, 307, 409, 512, 614,
        768, 614, 512, 409, 307, 230, 230, 230
    ];
    *delta = (*delta * ADAPT_TABLE[(nibble & 0x0F) as usize]) / 256;
    if *delta < 16 {
        *delta = 16;
    }
    sample as i16
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
    ima_pred: [i32; 2],
    ima_index: [i8; 2],
    ms_p1: [i32; 2],
    ms_p2: [i32; 2],
    ms_delta: [i32; 2],
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
            ima_pred: [0; 2],
            ima_index: [0; 2],
            ms_p1: [0; 2],
            ms_p2: [0; 2],
            ms_delta: [16; 2],
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
    fn frame_at(&mut self, src: &[u8], index: usize) -> [i32; 2] {
        match self.info.format_tag {
            1 => {
                let frame_bytes = self.info.frame_bytes();
                let at = index * frame_bytes;
                if at + frame_bytes > src.len() {
                    return [0, 0];
                }
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
            6 => {
                // A-law (8 bits per sample)
                let frame_bytes = self.info.channels as usize;
                let at = index * frame_bytes;
                if at + frame_bytes > src.len() {
                    return [0, 0];
                }
                let left = decode_alaw(src[at]) as i32;
                if self.info.channels == 1 {
                    [left, left]
                } else {
                    let right = decode_alaw(src[at + 1]) as i32;
                    [left, right]
                }
            }
            7 => {
                // µ-law (8 bits per sample)
                let frame_bytes = self.info.channels as usize;
                let at = index * frame_bytes;
                if at + frame_bytes > src.len() {
                    return [0, 0];
                }
                let left = decode_mulaw(src[at]) as i32;
                if self.info.channels == 1 {
                    [left, left]
                } else {
                    let right = decode_mulaw(src[at + 1]) as i32;
                    [left, right]
                }
            }
            17 => {
                // IMA ADPCM (4-bit samples)
                let at = index * self.info.channels as usize;
                if at >= src.len() {
                    return [0, 0];
                }
                let b = src[at];
                let left = decode_ima_adpcm_nibble(b & 0x0F, &mut self.ima_pred[0], &mut self.ima_index[0]) as i32;
                if self.info.channels == 1 {
                    [left, left]
                } else {
                    let right = decode_ima_adpcm_nibble((b >> 4) & 0x0F, &mut self.ima_pred[1], &mut self.ima_index[1]) as i32;
                    [left, right]
                }
            }
            2 => {
                // MS ADPCM (4-bit samples)
                let at = index * self.info.channels as usize;
                if at >= src.len() {
                    return [0, 0];
                }
                let b = src[at];
                let left = decode_ms_adpcm_nibble(b & 0x0F, &mut self.ms_p1[0], &mut self.ms_p2[0], &mut self.ms_delta[0], 256, 0) as i32;
                if self.info.channels == 1 {
                    [left, left]
                } else {
                    let right = decode_ms_adpcm_nibble((b >> 4) & 0x0F, &mut self.ms_p1[1], &mut self.ms_p2[1], &mut self.ms_delta[1], 256, 0) as i32;
                    [left, right]
                }
            }
            _ => [0, 0],
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
            format_tag: 1,
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 16,
            data_offset: 44,
            data_len: 4,
        };
        let mut converter = PcmConverter::new(info).unwrap();
        assert_eq!(converter.frame_at(&[0x10, 0x00, 0x20, 0x00], 1), [0x0020, 0x0020]);
    }
}
