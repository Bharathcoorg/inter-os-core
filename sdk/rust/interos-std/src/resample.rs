// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! PCM Audio Resampler targeting the platform's fixed 48 kHz stereo AC'97 audio stream.
//!
//! WHAT this is: an allocation-free, streaming audio resampler that converts 16-bit
//! signed PCM audio at arbitrary source sample rates (such as 44.1 kHz, 32 kHz, 22.05 kHz,
//! 16 kHz, 8 kHz, or 96 kHz) and channel counts (mono 1-channel or stereo 2-channel) into
//! the sound controller's native format: 48,000 Hz, 2 channels (stereo Left/Right), 16-bit
//! signed little-endian PCM.
//!
//! WHY it is needed: The AC'97 sound controller hardware in the emulator and physical target
//! runs at a fixed hardware sample rate of 48 kHz stereo. Passing audio at any other sample
//! rate directly would distort pitch and timing. This resampler enables audio playback for
//! music, video, games, and system sound effects without requiring hardware sample rate
//! converters or float runtime support.
//!
//! ZERO HEAP GUARANTEE:
//! This module is `#![no_std]` and performs zero heap allocations. Its state size is
//! bounded to a small stack-allocated struct holding a few previous sample words and
//! 64-bit phase registers.
//!
//! FIXED-POINT PHASE MATH:
//! All interpolation calculations use Q32.32 fixed-point phase math rather than IEEE-754
//! floating-point arithmetic. This prevents temporal drift, rounding jitter, and float
//! register churn in kernel/driver contexts.
//!
//! FRAMES VS SAMPLES:
//! - 1 source frame = `src_channels` samples (1 sample for mono, 2 samples for stereo).
//! - 1 target frame = 2 samples (Left and Right stereo).
//! - [`Resampler::process`] operates in whole frames and returns `(consumed_frames, written_frames)`.
//! - [`Resampler::process_samples`] provides a convenience view in terms of individual samples.

/// The target sample rate required by the AC'97 audio controller (48,000 Hz).
pub const TARGET_RATE: u32 = 48_000;

/// The target channel count (2 channels: Left and Right stereo).
pub const TARGET_CHANNELS: usize = 2;

/// Fixed-point one (1.0 in Q32.32 format).
const Q32_ONE: u64 = 1 << 32;

/// Rounding bias for Q32.32 fixed-point division (0.5 in Q32.32 format).
const Q32_HALF: i64 = 0x8000_0000;

/// Errors that can occur when configuring the resampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResampleError {
    /// Sample rate must be non-zero and within a valid audio range (1..=384,000 Hz).
    InvalidSampleRate,
    /// Channel count must be 1 (mono) or 2 (stereo).
    UnsupportedChannels(u8),
}

/// An allocation-free streaming PCM audio resampler.
#[derive(Clone, Copy, Debug)]
pub struct Resampler {
    src_rate: u32,
    src_channels: u8,
    /// Source frame step per output frame in Q32.32 fixed point.
    step: u64,
    /// Current fractional frame phase in Q32.32 fixed point.
    phase: u64,
    /// Carried frame from the end of the previous input block: `[Left, Right]`.
    /// For mono input streams, Left == Right.
    carry: [i16; 2],
    /// Whether `carry` holds a valid frame from the preceding block.
    has_carry: bool,
    /// Whether `finish()` has been called to signal the end of the input stream.
    finished: bool,
}

impl Resampler {
    /// Creates a new resampler converting from `src_rate` and `src_channels` to 48 kHz stereo.
    ///
    /// # Errors
    /// Returns [`ResampleError::InvalidSampleRate`] if `src_rate == 0` or `src_rate > 384_000`.
    /// Returns [`ResampleError::UnsupportedChannels`] if `src_channels` is neither 1 (mono) nor 2 (stereo).
    pub fn new(src_rate: u32, src_channels: u8) -> Result<Self, ResampleError> {
        if src_rate == 0 || src_rate > 384_000 {
            return Err(ResampleError::InvalidSampleRate);
        }
        if src_channels != 1 && src_channels != 2 {
            return Err(ResampleError::UnsupportedChannels(src_channels));
        }

        let step = if src_rate == TARGET_RATE {
            Q32_ONE
        } else {
            ((src_rate as u64) << 32) / (TARGET_RATE as u64)
        };

        Ok(Self {
            src_rate,
            src_channels,
            step,
            phase: 0,
            carry: [0; 2],
            has_carry: false,
            finished: false,
        })
    }

    /// Convenience constructor for mono streams.
    pub fn new_mono(src_rate: u32) -> Result<Self, ResampleError> {
        Self::new(src_rate, 1)
    }

    /// Convenience constructor for stereo streams.
    pub fn new_stereo(src_rate: u32) -> Result<Self, ResampleError> {
        Self::new(src_rate, 2)
    }

    /// The source sample rate in Hz.
    pub const fn src_rate(&self) -> u32 {
        self.src_rate
    }

    /// The source channel count (1 = mono, 2 = stereo).
    pub const fn src_channels(&self) -> u8 {
        self.src_channels
    }

    /// The fixed target sample rate (48,000 Hz).
    pub const fn target_rate(&self) -> u32 {
        TARGET_RATE
    }

    /// The fixed target channel count (2 channels, stereo).
    pub const fn target_channels(&self) -> usize {
        TARGET_CHANNELS
    }

    /// Returns `true` if the source is already 48 kHz stereo and needs no interpolation or channel expansion.
    pub const fn is_passthrough(&self) -> bool {
        self.src_rate == TARGET_RATE && self.src_channels == 2
    }

    /// Resets the internal phase and history state, allowing reuse of this instance for a new stream.
    pub fn reset(&mut self) {
        self.phase = 0;
        self.carry = [0; 2];
        self.has_carry = false;
        self.finished = false;
    }

    /// Signals that no further input frames will be pushed for this stream.
    ///
    /// Subsequent calls to [`process`](Self::process) or [`drain`](Self::drain) will
    /// flush any remaining fractional frame.
    pub fn finish(&mut self) {
        self.finished = true;
    }

    /// Returns `true` if [`finish`](Self::finish) has been called.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Returns `true` if the stream was marked finished and all pending frames have been drained.
    pub fn is_drained(&self) -> bool {
        self.finished && !self.has_carry
    }

    /// Resamples as much of `input` as will fit into `output`.
    ///
    /// - `input` contains interleaved 16-bit PCM samples with `self.src_channels` per frame.
    /// - `output` receives interleaved 16-bit stereo PCM samples (2 channels: `[Left, Right]`).
    ///
    /// Returns `(consumed_frames, written_frames)`:
    /// - `consumed_frames`: whole source frames consumed from `input`. The caller should
    ///   advance their input slice by `consumed_frames * self.src_channels` samples.
    /// - `written_frames`: whole stereo frames written to `output`. The caller should
    ///   advance their output slice by `written_frames * 2` samples.
    pub fn process(&mut self, input: &[i16], output: &mut [i16]) -> (usize, usize) {
        let ch = self.src_channels as usize;
        let in_frames = input.len() / ch;
        let out_frames = output.len() / TARGET_CHANNELS;

        if out_frames == 0 {
            return (0, 0);
        }

        if in_frames == 0 {
            if self.finished && self.has_carry && out_frames > 0 {
                output[0] = self.carry[0];
                output[1] = self.carry[1];
                self.has_carry = false;
                return (0, 1);
            }
            return (0, 0);
        }

        // Fast path: exact 48 kHz input rate.
        if self.src_rate == TARGET_RATE {
            let count = in_frames.min(out_frames);
            if count == 0 {
                return (0, 0);
            }
            if ch == 2 {
                // Stereo -> Stereo: exact bit identity copy.
                output[..count * 2].copy_from_slice(&input[..count * 2]);
            } else {
                // Mono -> Stereo: duplicate L to both L and R.
                for i in 0..count {
                    let s = input[i];
                    output[i * 2] = s;
                    output[i * 2 + 1] = s;
                }
            }
            return (count, count);
        }

        // Streaming resampling path using Q32.32 fixed-point linear interpolation.
        let mut written = 0usize;
        let mut pos = self.phase;

        if !self.has_carry {
            if in_frames == 0 {
                return (0, 0);
            }

            while written < out_frames {
                let idx = (pos >> 32) as usize;
                let frac = (pos & 0xFFFF_FFFF) as u32;

                if idx + 1 >= in_frames {
                    // At the very beginning (pos == 0), the first output frame is exact frame 0.
                    if idx == 0 && frac == 0 && in_frames >= 1 {
                        let frame = self.frame_at(input, 0);
                        output[written * 2] = frame[0];
                        output[written * 2 + 1] = frame[1];
                        written += 1;
                        pos = pos.saturating_add(self.step);
                        continue;
                    }
                    break;
                }

                let before = self.frame_at(input, idx);
                let after = self.frame_at(input, idx + 1);
                output[written * 2] = interpolate_sample(before[0], after[0], frac);
                output[written * 2 + 1] = interpolate_sample(before[1], after[1], frac);
                written += 1;
                pos = pos.saturating_add(self.step);
            }

            let stopped_at = ((pos >> 32) as usize).min(in_frames);
            if written == out_frames && stopped_at < in_frames.saturating_sub(1) {
                self.phase = pos.saturating_sub((stopped_at as u64) << 32);
                (stopped_at, written)
            } else if stopped_at >= in_frames.saturating_sub(1) && in_frames > 0 {
                self.carry = self.frame_at(input, in_frames - 1);
                self.has_carry = true;
                self.phase = pos.saturating_sub(((in_frames - 1) as u64) << 32);
                (in_frames, written)
            } else {
                self.phase = pos.saturating_sub((stopped_at as u64) << 32);
                (stopped_at, written)
            }
        } else {
            while written < out_frames {
                let idx = (pos >> 32) as usize;
                let frac = (pos & 0xFFFF_FFFF) as u32;

                if idx >= in_frames {
                    // When finished, drain any remaining fractional frame holding carry.
                    if self.finished && idx == 0 && frac < (Q32_ONE as u32) {
                        output[written * 2] = self.carry[0];
                        output[written * 2 + 1] = self.carry[1];
                        written += 1;
                        pos = pos.saturating_add(self.step);
                        self.has_carry = false;
                    }
                    break;
                }

                let before = if idx == 0 {
                    self.carry
                } else {
                    self.frame_at(input, idx - 1)
                };
                let after = self.frame_at(input, idx);

                output[written * 2] = interpolate_sample(before[0], after[0], frac);
                output[written * 2 + 1] = interpolate_sample(before[1], after[1], frac);
                written += 1;
                pos = pos.saturating_add(self.step);
            }

            let stopped_at = ((pos >> 32) as usize).min(in_frames);
            if in_frames > 0 && stopped_at >= in_frames {
                self.carry = self.frame_at(input, in_frames - 1);
                self.phase = pos.saturating_sub((in_frames as u64) << 32);
                (in_frames, written)
            } else {
                if stopped_at > 0 {
                    self.carry = self.frame_at(input, stopped_at - 1);
                }
                self.phase = pos.saturating_sub((stopped_at as u64) << 32);
                (stopped_at, written)
            }
        }
    }

    /// Convenience wrapper around [`process`](Self::process) operating in sample counts.
    ///
    /// Returns `(consumed_samples, written_samples)`.
    pub fn process_samples(&mut self, input: &[i16], output: &mut [i16]) -> (usize, usize) {
        let (frames_in, frames_out) = self.process(input, output);
        (
            frames_in * (self.src_channels as usize),
            frames_out * TARGET_CHANNELS,
        )
    }

    /// Drains any remaining fractional output frame after [`finish`](Self::finish) has been called.
    ///
    /// Returns the number of stereo frames written to `output` (0 or 1).
    pub fn drain(&mut self, output: &mut [i16]) -> usize {
        self.finish();
        let out_frames = output.len() / TARGET_CHANNELS;
        if self.has_carry && out_frames > 0 {
            output[0] = self.carry[0];
            output[1] = self.carry[1];
            self.has_carry = false;
            1
        } else {
            0
        }
    }

    /// Marks the stream as finished and drains any remaining frame in a single call.
    pub fn finish_and_drain(&mut self, output: &mut [i16]) -> usize {
        self.drain(output)
    }

    /// Helper to read an input frame into signed stereo `[Left, Right]` samples.
    #[inline(always)]
    fn frame_at(&self, input: &[i16], index: usize) -> [i16; 2] {
        if self.src_channels == 1 {
            let s = input[index];
            [s, s]
        } else {
            let base = index * 2;
            [input[base], input[base + 1]]
        }
    }
}

/// Computes linear interpolation between two 16-bit PCM samples with rounding:
/// `before + round((after - before) * frac / 2^32)`
#[inline(always)]
fn interpolate_sample(before: i16, after: i16, frac_q32: u32) -> i16 {
    let b = before as i64;
    let a = after as i64;
    let diff = a - b;
    let product = diff * (frac_q32 as i64);
    let rounded = (product + Q32_HALF) >> 32;
    (b + rounded).clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_configuration_is_refused() {
        assert_eq!(
            Resampler::new(0, 2).err(),
            Some(ResampleError::InvalidSampleRate)
        );
        assert_eq!(
            Resampler::new(500_000, 2).err(),
            Some(ResampleError::InvalidSampleRate)
        );
        assert_eq!(
            Resampler::new(44_100, 0).err(),
            Some(ResampleError::UnsupportedChannels(0))
        );
        assert_eq!(
            Resampler::new(44_100, 3).err(),
            Some(ResampleError::UnsupportedChannels(3))
        );
    }

    #[test]
    fn exact_48khz_stereo_passthrough_matches_bit_for_bit() {
        let mut resampler = Resampler::new_stereo(48_000).unwrap();
        assert!(resampler.is_passthrough());

        let input: [i16; 8] = [100, -200, 300, -400, 500, -600, 700, -800];
        let mut output = [0i16; 8];

        let (consumed, written) = resampler.process(&input, &mut output);
        assert_eq!(consumed, 4);
        assert_eq!(written, 4);
        assert_eq!(output, input);
    }

    #[test]
    fn exact_48khz_mono_duplicates_to_stereo() {
        let mut resampler = Resampler::new_mono(48_000).unwrap();
        assert!(!resampler.is_passthrough());

        let input: [i16; 4] = [123, -456, 789, -1000];
        let mut output = [0i16; 8];

        let (consumed, written) = resampler.process(&input, &mut output);
        assert_eq!(consumed, 4);
        assert_eq!(written, 4);
        assert_eq!(output, [123, 123, -456, -456, 789, 789, -1000, -1000]);
    }

    #[test]
    fn dc_level_is_strictly_preserved() {
        let mut resampler = Resampler::new_stereo(44_100).unwrap();
        let input = [1234i16, -5678i16];
        let mut output = [0i16; 4];

        let (_, written) = resampler.process(&input, &mut output);
        assert_eq!(written, 1);
        assert_eq!(output[0], 1234);
        assert_eq!(output[1], -5678);
    }
}
