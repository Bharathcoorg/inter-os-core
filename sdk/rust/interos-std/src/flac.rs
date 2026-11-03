// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Native FLAC decoding into interleaved signed 16-bit PCM.
//!
//! WHAT this is: a bounded, streaming decoder for native (`fLaC`) FLAC
//! bitstreams. It is written for the Music player, which reads a file through
//! a capability-gated filesystem in 2 KiB pieces and cannot seek. The decoder
//! owns all of its state; nothing is allocated, and no floating point is used.
//!
//! # How the caller drives it
//!
//! ```ignore
//! let mut decoder = interos_std::flac::FlacDecoder::new();
//! loop {
//!     let n = decoder.push(&chunk[..read]);
//!     // n may be smaller than read if the internal buffer is full; push the
//!     // rest after draining frames with next_frame.
//!     while let Some(frame) = decoder.next_frame(&mut pcm)? {
//!         // pcm[..frame.samples] holds interleaved i16 samples.
//!     }
//!     if at_eof {
//!         decoder.finish();
//!         while let Some(frame) = decoder.next_frame(&mut pcm)? { /* ... */ }
//!         break;
//!     }
//! }
//! ```
//!
//! `push` copies bytes into a fixed 32 KiB buffer and returns how many it took.
//! `next_frame` returns `Ok(Some(info))` for one decoded frame, `Ok(None)` when
//! it needs more input (or the stream ended cleanly), and a typed error for a
//! stream it refuses. Calling `finish` is what turns "the file ended in the
//! middle of a frame" into [`FlacError::Truncated`] instead of "need more".
//!
//! # Output
//!
//! Samples are interleaved left/right (mono stays mono) at the stream's own
//! rate; no resampling is done here. [`FrameInfo`] carries the rate, channel
//! count, block size and stream position so the player can convert for the
//! controller. The output buffer must hold `block_size * channels` samples;
//! otherwise [`FlacError::OutputTooSmall`] is returned and the frame is kept.
//!
//! A stream whose STREAMINFO declares more than 16 bits is accepted down to
//! 16 bits by an arithmetic right shift (24-bit loses its low 8 bits, 20-bit
//! its low 4). This is truncation, not dithering or a gain change, and it is
//! tested. Streams shallower than 16 bits are shifted left to fill the range.
//!
//! # What is refused
//!
//! - Ogg-encapsulated FLAC ([`FlacError::OggEncapsulated`]); only the native
//!   `fLaC` stream is decoded.
//! - 32-bit samples ([`FlacError::UnsupportedSampleSize`]). FLAC 1.4's
//!   32-bit integers and the float convention layered on them are not
//!   down-shifted silently; both stop at STREAMINFO/frame-header validation.
//! - More than two channels, block sizes above 4608 (the FLAC subset limit),
//!   unknown sample rates.
//! - Frames larger than the 32 KiB input buffer, i.e. streams that cannot be
//!   decoded with bounded memory.
//! - Corrupt input with distinct errors for a bad header CRC, a bad frame
//!   CRC, structural corruption, and truncation. A truncated file never
//!   yields a partial frame.
//!
//! The decoder does not verify the STREAMINFO MD5 or the optional seek table;
//! it verifies every frame CRC and reports the frames it actually decoded.
//! Variable-blocking streams are parsed, but their sample numbers are not
//! used for seeking because this decoder cannot seek.
//!
//! # Memory
//!
//! [`FlacDecoder`] is about 68 KiB (32 KiB of input plus two 4608-sample i32
//! channel buffers) and should live in static storage on the guest, not be
//! copied around the stack.

/// Largest block size this decoder accepts (FLAC subset limit for <=48 kHz).
pub const MAX_BLOCK_SIZE: usize = 4_608;

/// Channels this decoder accepts. Multichannel FLAC is refused, not mixed.
pub const MAX_CHANNELS: usize = 2;

/// Largest encoded frame the streaming buffer can hold.
pub const MAX_FRAME_BYTES: usize = 32 * 1024;

/// The inter-channel transform a frame's two subframes use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelAssignment {
    /// One or two independently coded channels.
    Independent,
    /// Channel 0 is left, channel 1 is left-minus-right.
    LeftSide,
    /// Channel 0 is left-minus-right, channel 1 is right.
    RightSide,
    /// Channel 0 is (left+right)>>1, channel 1 is left-minus-right.
    MidSide,
}

/// The fixed STREAMINFO facts about a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Smallest block size in the stream.
    pub min_block_size: u16,
    /// Largest block size in the stream.
    pub max_block_size: u16,
    /// Smallest encoded frame size, 0 when unknown.
    pub min_frame_size: u32,
    /// Largest encoded frame size, 0 when unknown.
    pub max_frame_size: u32,
    /// Samples per second.
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u8,
    /// 4..=24.
    pub bits_per_sample: u8,
    /// Total frames of audio, 0 when the encoder did not know.
    pub total_samples: u64,
}

/// What one decoded frame contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Samples per second of the stream.
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u8,
    /// Bits per sample of the source (not of the emitted i16).
    pub bits_per_sample: u8,
    /// Frames of audio in this block.
    pub block_size: u32,
    /// Position of this block's first frame in the stream.
    pub first_sample: u64,
    /// Number of interleaved i16 samples written to the output buffer.
    pub samples: usize,
    /// Which stereo transform this frame used.
    pub channel_assignment: ChannelAssignment,
}

/// Why a stream could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlacError {
    /// The first bytes are neither `fLaC` nor `OggS`.
    NotFlac,
    /// The input is Ogg-encapsulated FLAC, which is not a native stream.
    OggEncapsulated,
    /// The input ended inside a metadata block, frame, or before all the
    /// samples STREAMINFO promised.
    Truncated,
    /// A frame did not fit the bounded input buffer.
    FrameTooLarge,
    /// The metadata chain carried no STREAMINFO block.
    MissingStreamInfo,
    /// A metadata block was malformed (bad size, duplicate STREAMINFO, ...).
    CorruptMetadata,
    /// The frame header's CRC-8 does not match.
    CorruptHeaderCrc,
    /// The frame's CRC-16 does not match.
    CorruptFrameCrc,
    /// The frame is structurally invalid (reserved fields, bad partition,
    /// bad coded number, ...).
    CorruptFrame,
    /// Sample width this decoder refuses (32-bit streams stop here).
    UnsupportedSampleSize(u8),
    /// Sample rate of zero or outside FLAC's range for this decoder.
    UnsupportedSampleRate(u32),
    /// More than two channels.
    UnsupportedChannelCount(u8),
    /// A block size above [`MAX_BLOCK_SIZE`].
    UnsupportedBlockSize(u32),
    /// The caller's output slice cannot hold the whole block.
    OutputTooSmall {
        /// Interleaved samples the frame needs.
        needed: usize,
    },
}

impl FlacError {
    /// True for the errors that mean the bytes are damaged, as opposed to a
    /// format this decoder does not support.
    pub fn is_corrupt(self) -> bool {
        matches!(
            self,
            FlacError::CorruptHeaderCrc
                | FlacError::CorruptFrameCrc
                | FlacError::CorruptFrame
                | FlacError::CorruptMetadata
        )
    }
}

/// A bounded streaming FLAC decoder.
pub struct FlacDecoder {
    buf: [u8; MAX_FRAME_BYTES],
    start: usize,
    end: usize,
    stage: Stage,
    metadata_last: bool,
    info: Option<StreamInfo>,
    decoded_samples: u64,
    samples: [[i32; MAX_BLOCK_SIZE]; MAX_CHANNELS],
    eof: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Expecting the `fLaC` marker.
    Marker,
    /// Expecting a 4-byte metadata block header.
    BlockHeader,
    /// Reading the 34-byte STREAMINFO body.
    StreamInfo(usize),
    /// Skipping a metadata block body.
    Skip(usize),
    /// Metadata is done; frames follow.
    Frames,
}

impl FlacDecoder {
    /// Creates an empty decoder. The state is bounded and const-initialisable,
    /// so a guest may keep it in static memory.
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_FRAME_BYTES],
            start: 0,
            end: 0,
            stage: Stage::Marker,
            metadata_last: false,
            info: None,
            decoded_samples: 0,
            samples: [[0; MAX_BLOCK_SIZE]; MAX_CHANNELS],
            eof: false,
        }
    }

    /// Offers bytes read sequentially from the file.
    ///
    /// Returns the number of bytes accepted, which is less than `bytes.len()`
    /// only when the bounded buffer is full. The caller should drain frames
    /// with [`next_frame`](Self::next_frame) and push the remainder. Bytes
    /// offered after [`finish`](Self::finish) are refused (0 returned).
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        if self.eof || bytes.is_empty() {
            return 0;
        }
        let mut room = MAX_FRAME_BYTES - self.end;
        if bytes.len() > room && self.start > 0 {
            let len = self.end - self.start;
            self.buf.copy_within(self.start..self.end, 0);
            self.start = 0;
            self.end = len;
            room = MAX_FRAME_BYTES - self.end;
        }
        let take = bytes.len().min(room);
        self.buf[self.end..self.end + take].copy_from_slice(&bytes[..take]);
        self.end += take;
        take
    }

    /// Declares that the file has ended. Afterwards a partial metadata block,
    /// frame, or missing promised samples are reported as
    /// [`FlacError::Truncated`].
    pub fn finish(&mut self) {
        self.eof = true;
    }

    /// The stream description, available once STREAMINFO has been read.
    pub fn stream_info(&self) -> Option<StreamInfo> {
        self.info
    }

    /// Decodes the next frame into `out`.
    ///
    /// `Ok(Some(info))` means `out[..info.samples]` holds the frame.
    /// `Ok(None)` means more input is needed, or the stream ended cleanly.
    /// Errors never leave a partial frame for the caller to mistake for audio.
    pub fn next_frame(&mut self, out: &mut [i16]) -> Result<Option<FrameInfo>, FlacError> {
        // Walk the metadata chain first. Each step either makes progress or
        // reports that it needs bytes that have not arrived yet.
        while self.stage != Stage::Frames {
            match self.metadata_step() {
                Ok(()) => continue,
                Err(Step::NeedMore) => {
                    if self.eof {
                        return Err(FlacError::Truncated);
                    }
                    return Ok(None);
                }
                Err(Step::Fatal(error)) => return Err(error),
            }
        }

        let info = self.info.ok_or(FlacError::MissingStreamInfo)?;
        if self.buf_len() == 0 {
            if !self.eof {
                return Ok(None);
            }
            if info.total_samples != 0 && self.decoded_samples < info.total_samples {
                return Err(FlacError::Truncated);
            }
            return Ok(None);
        }

        let (start, end) = (self.start, self.end);
        match decode_frame(
            &self.buf[start..end],
            &info,
            out,
            &mut self.samples,
        ) {
            Ok((frame, consumed)) => {
                if info.total_samples != 0
                    && self.decoded_samples + frame.block_size as u64 > info.total_samples
                {
                    return Err(FlacError::CorruptFrame);
                }
                self.start += consumed;
                let first_sample = self.decoded_samples;
                self.decoded_samples += frame.block_size as u64;
                Ok(Some(FrameInfo {
                    sample_rate: frame.sample_rate,
                    channels: frame.channels,
                    bits_per_sample: frame.bits_per_sample,
                    block_size: frame.block_size,
                    first_sample,
                    samples: frame.block_size as usize * frame.channels as usize,
                    channel_assignment: frame.channel_assignment,
                }))
            }
            Err(Step::NeedMore) => {
                // A complete frame that still does not fit cannot be decoded
                // with bounded memory, no matter how many bytes arrive.
                if self.buf_len() == MAX_FRAME_BYTES {
                    return Err(FlacError::FrameTooLarge);
                }
                if self.eof {
                    return Err(FlacError::Truncated);
                }
                Ok(None)
            }
            Err(Step::Fatal(error)) => Err(error),
        }
    }

    fn buf_len(&self) -> usize {
        self.end - self.start
    }

    /// Advances the metadata state machine by one step.
    fn metadata_step(&mut self) -> Result<(), Step> {
        match self.stage {
            Stage::Marker => {
                if self.buf_len() < 4 {
                    return Err(Step::NeedMore);
                }
                let tag = &self.buf[self.start..self.start + 4];
                if tag == b"OggS" {
                    return Err(Step::Fatal(FlacError::OggEncapsulated));
                }
                if tag != b"fLaC" {
                    return Err(Step::Fatal(FlacError::NotFlac));
                }
                self.start += 4;
                self.stage = Stage::BlockHeader;
                Ok(())
            }
            Stage::BlockHeader => {
                if self.buf_len() < 4 {
                    return Err(Step::NeedMore);
                }
                let header = self.buf[self.start];
                let block_type = header & 0x7f;
                self.metadata_last = header & 0x80 != 0;
                let length = ((self.buf[self.start + 1] as usize) << 16)
                    | ((self.buf[self.start + 2] as usize) << 8)
                    | self.buf[self.start + 3] as usize;
                self.start += 4;
                if block_type == 0 {
                    if length != 34 || self.info.is_some() {
                        return Err(Step::Fatal(FlacError::CorruptMetadata));
                    }
                    self.stage = Stage::StreamInfo(34);
                } else if block_type == 127 {
                    return Err(Step::Fatal(FlacError::CorruptMetadata));
                } else {
                    self.stage = Stage::Skip(length);
                    if length == 0 {
                        self.close_metadata_block()?;
                    }
                }
                Ok(())
            }
            Stage::StreamInfo(remaining) => {
                if self.buf_len() < remaining {
                    return Err(Step::NeedMore);
                }
                let info = parse_stream_info(&self.buf[self.start..self.start + 34])?;
                self.start += 34;
                self.info = Some(info);
                self.close_metadata_block()?;
                Ok(())
            }
            Stage::Skip(remaining) => {
                let take = remaining.min(self.buf_len());
                self.start += take;
                if take == remaining {
                    self.close_metadata_block()?;
                } else if take == 0 {
                    return Err(Step::NeedMore);
                } else {
                    self.stage = Stage::Skip(remaining - take);
                }
                Ok(())
            }
            Stage::Frames => Ok(()),
        }
    }

    /// Reacts to the last-block flag: either more metadata follows, or the
    /// frame section starts (and a STREAMINFO must have been seen).
    fn close_metadata_block(&mut self) -> Result<(), Step> {
        if self.metadata_last {
            if self.info.is_none() {
                return Err(Step::Fatal(FlacError::MissingStreamInfo));
            }
            self.stage = Stage::Frames;
        } else {
            self.stage = Stage::BlockHeader;
        }
        Ok(())
    }
}

impl Default for FlacDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal control flow: either the buffered bytes ran out mid-parse, or the
/// stream is refused.
#[derive(Debug)]
enum Step {
    NeedMore,
    Fatal(FlacError),
}

impl From<ReadError> for Step {
    fn from(_: ReadError) -> Self {
        Step::NeedMore
    }
}

fn fatal(error: FlacError) -> Step {
    Step::Fatal(error)
}

fn parse_stream_info(bytes: &[u8]) -> Result<StreamInfo, Step> {
    debug_assert_eq!(bytes.len(), 34);
    let min_block_size = u16::from_be_bytes([bytes[0], bytes[1]]);
    let max_block_size = u16::from_be_bytes([bytes[2], bytes[3]]);
    let min_frame_size = ((bytes[4] as u32) << 16) | ((bytes[5] as u32) << 8) | bytes[6] as u32;
    let max_frame_size = ((bytes[7] as u32) << 16) | ((bytes[8] as u32) << 8) | bytes[9] as u32;
    let packed = u64::from_be_bytes([
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
    ]);
    let sample_rate = ((packed >> 44) & 0x000f_ffff) as u32;
    let channels = (((packed >> 41) & 0x7) + 1) as u8;
    let bits_per_sample = (((packed >> 36) & 0x1f) + 1) as u8;
    let total_samples = packed & 0x0000_000f_ffff_ffff;

    if min_block_size == 0 || max_block_size == 0 || max_block_size < min_block_size {
        return Err(fatal(FlacError::CorruptMetadata));
    }
    if sample_rate == 0 || sample_rate > 655_350 {
        return Err(fatal(FlacError::UnsupportedSampleRate(sample_rate)));
    }
    if channels > MAX_CHANNELS as u8 {
        return Err(fatal(FlacError::UnsupportedChannelCount(channels)));
    }
    if !(4..=24).contains(&bits_per_sample) {
        return Err(fatal(FlacError::UnsupportedSampleSize(bits_per_sample)));
    }
    Ok(StreamInfo {
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
    })
}

struct DecodedFrame {
    sample_rate: u32,
    channels: u8,
    bits_per_sample: u8,
    block_size: u32,
    channel_assignment: ChannelAssignment,
}

/// Decodes one frame from the front of `data`. On success returns the frame
/// description and the number of bytes it occupied; no bytes are consumed by
/// the caller until the frame (including CRC-16) is complete and valid.
fn decode_frame(
    data: &[u8],
    info: &StreamInfo,
    out: &mut [i16],
    scratch: &mut [[i32; MAX_BLOCK_SIZE]; MAX_CHANNELS],
) -> Result<(DecodedFrame, usize), Step> {
    let mut reader = BitReader::new(data);

    let sync = reader.read_bits(14)?;
    if sync != 0x3ffe {
        return Err(fatal(FlacError::CorruptFrame));
    }
    if reader.read_bit()? != 0 {
        return Err(fatal(FlacError::CorruptFrame));
    }
    let _variable_blocking = reader.read_bit()?;
    let block_size_code = reader.read_bits(4)?;
    let sample_rate_code = reader.read_bits(4)?;
    let channel_code = reader.read_bits(4)?;
    let sample_size_code = reader.read_bits(3)?;
    if reader.read_bit()? != 0 {
        return Err(fatal(FlacError::CorruptFrame));
    }

    // UTF-8-style coded frame/sample number. We do not seek, so the value is
    // validated and discarded; bad continuation bytes are corruption.
    let lead = reader.read_bits(8)? as u8;
    let continuation = match lead {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        0xf8..=0xfb => 4,
        0xfc..=0xfd => 5,
        0xfe => 6,
        _ => return Err(fatal(FlacError::CorruptFrame)),
    };
    for _ in 0..continuation {
        if reader.read_bits(8)? as u8 & 0xc0 != 0x80 {
            return Err(fatal(FlacError::CorruptFrame));
        }
    }

    let block_size: u32 = match block_size_code {
        0 => return Err(fatal(FlacError::CorruptFrame)),
        1 => 192,
        2..=5 => 576u32 << (block_size_code - 2),
        6 => reader.read_bits(8)? + 1,
        7 => reader.read_bits(16)? + 1,
        8..=15 => 256u32 << (block_size_code - 8),
        _ => unreachable!(),
    };
    if block_size == 0 || block_size as usize > MAX_BLOCK_SIZE {
        return Err(fatal(FlacError::UnsupportedBlockSize(block_size)));
    }

    let sample_rate: u32 = match sample_rate_code {
        0 => info.sample_rate,
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => reader.read_bits(8)? * 1_000,
        13 => reader.read_bits(16)?,
        14 => reader.read_bits(16)? * 10,
        _ => return Err(fatal(FlacError::CorruptFrame)),
    };
    if sample_rate == 0 {
        return Err(fatal(FlacError::UnsupportedSampleRate(0)));
    }
    if sample_rate != info.sample_rate {
        return Err(fatal(FlacError::CorruptFrame));
    }

    let bits_per_sample: u8 = match sample_size_code {
        0 => info.bits_per_sample,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => return Err(fatal(FlacError::UnsupportedSampleSize(32))),
        _ => return Err(fatal(FlacError::CorruptFrame)),
    };
    if bits_per_sample != info.bits_per_sample {
        return Err(fatal(FlacError::CorruptFrame));
    }

    let (channels, channel_assignment) = match channel_code {
        0..=7 => (channel_code as u8 + 1, ChannelAssignment::Independent),
        8 => (2, ChannelAssignment::LeftSide),
        9 => (2, ChannelAssignment::RightSide),
        10 => (2, ChannelAssignment::MidSide),
        _ => return Err(fatal(FlacError::CorruptFrame)),
    };
    if channels as usize > MAX_CHANNELS || channels != info.channels {
        return Err(fatal(FlacError::CorruptFrame));
    }

    // Header CRC-8 covers every header byte up to, but not including, itself.
    reader.align();
    let header_len = reader.byte_position();
    let stored_header_crc = reader.read_bits(8)? as u8;
    if crc8(&data[..header_len]) != stored_header_crc {
        return Err(fatal(FlacError::CorruptHeaderCrc));
    }

    let needed = block_size as usize * channels as usize;
    if out.len() < needed {
        return Err(fatal(FlacError::OutputTooSmall { needed }));
    }

    // Side channels carry one extra bit of range; mid channels keep the
    // stream's width. Wasted bits are handled inside decode_subframe.
    let channel_bits: [u32; MAX_CHANNELS] = match channel_assignment {
        ChannelAssignment::Independent => {
            [bits_per_sample as u32, bits_per_sample as u32]
        }
        ChannelAssignment::LeftSide | ChannelAssignment::MidSide => {
            [bits_per_sample as u32, bits_per_sample as u32 + 1]
        }
        ChannelAssignment::RightSide => {
            [bits_per_sample as u32 + 1, bits_per_sample as u32]
        }
    };

    for channel in 0..channels as usize {
        decode_subframe(
            &mut reader,
            channel_bits[channel],
            block_size as usize,
            &mut scratch[channel],
        )?;
    }

    if channel_assignment != ChannelAssignment::Independent {
        apply_stereo(channel_assignment, scratch, block_size as usize);
    }

    let bps = bits_per_sample;
    for index in 0..block_size as usize {
        let at = index * channels as usize;
        for channel in 0..channels as usize {
            out[at + channel] = to_i16(scratch[channel][index], bps);
        }
    }

    // Subframes end on a byte boundary; the remaining bits are padding.
    reader.align();
    let frame_data_len = reader.byte_position();
    let stored_frame_crc = reader.read_bits(16)? as u16;
    if crc16(&data[..frame_data_len]) != stored_frame_crc {
        return Err(fatal(FlacError::CorruptFrameCrc));
    }

    Ok((
        DecodedFrame {
            sample_rate,
            channels,
            bits_per_sample,
            block_size,
            channel_assignment,
        },
        frame_data_len + 2,
    ))
}

/// Decodes one subframe into `samples[..block_size]`.
fn decode_subframe(
    reader: &mut BitReader<'_>,
    channel_bits: u32,
    block_size: usize,
    samples: &mut [i32],
) -> Result<(), Step> {
    if reader.read_bit()? != 0 {
        return Err(fatal(FlacError::CorruptFrame));
    }
    let kind = reader.read_bits(6)? as u8;
    let wasted = if reader.read_bit()? != 0 {
        reader.read_unary()? + 1
    } else {
        0
    };
    if wasted >= channel_bits {
        return Err(fatal(FlacError::CorruptFrame));
    }
    let bits = channel_bits - wasted;
    let out = &mut samples[..block_size];

    match kind {
        0 => {
            let value = reader.read_signed(bits)?;
            out.fill(value);
        }
        1 => {
            for sample in out.iter_mut() {
                *sample = reader.read_signed(bits)?;
            }
        }
        8..=12 => {
            let order = (kind & 7) as usize;
            if order > block_size {
                return Err(fatal(FlacError::CorruptFrame));
            }
            for sample in out[..order].iter_mut() {
                *sample = reader.read_signed(bits)?;
            }
            decode_residual(reader, block_size, order, &mut out[order..])?;
            restore_fixed(out, order)?;
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > block_size {
                return Err(fatal(FlacError::CorruptFrame));
            }
            for sample in out[..order].iter_mut() {
                *sample = reader.read_signed(bits)?;
            }
            let precision_code = reader.read_bits(4)?;
            if precision_code == 15 {
                return Err(fatal(FlacError::CorruptFrame));
            }
            let precision = precision_code + 1;
            let shift = reader.read_signed(5)?;
            if shift < 0 {
                return Err(fatal(FlacError::CorruptFrame));
            }
            let mut coefficients = [0i32; 32];
            for coefficient in coefficients[..order].iter_mut() {
                *coefficient = reader.read_signed(precision)?;
            }
            decode_residual(reader, block_size, order, &mut out[order..])?;
            restore_lpc(out, order, &coefficients, shift as u32)?;
        }
        _ => return Err(fatal(FlacError::CorruptFrame)),
    }

    if wasted > 0 {
        // The stored values use `bits` bits; shifting by the wasted count
        // restores the full-width sample without ever exceeding i32.
        let factor = 1i64 << wasted;
        for sample in out.iter_mut() {
            *sample = (*sample as i64 * factor) as i32;
        }
    }
    Ok(())
}

/// Decodes the Rice-coded residual for one subframe into `residual`.
fn decode_residual(
    reader: &mut BitReader<'_>,
    block_size: usize,
    order: usize,
    residual: &mut [i32],
) -> Result<(), Step> {
    let method = reader.read_bits(2)?;
    let parameter_bits = match method {
        0 => 4,
        1 => 5,
        _ => return Err(fatal(FlacError::CorruptFrame)),
    };
    let escape = (1u32 << parameter_bits) - 1;
    let partition_order = reader.read_bits(4)?;
    let partitions = 1usize << partition_order;
    if block_size % partitions != 0 {
        return Err(fatal(FlacError::CorruptFrame));
    }
    let partition_len = block_size / partitions;
    if partition_len < order {
        return Err(fatal(FlacError::CorruptFrame));
    }

    let mut written = 0usize;
    for partition in 0..partitions {
        let count = if partition == 0 {
            partition_len - order
        } else {
            partition_len
        };
        let parameter = reader.read_bits(parameter_bits)?;
        if parameter == escape {
            let raw_bits = reader.read_bits(5)?;
            if raw_bits == 0 {
                for _ in 0..count {
                    residual[written] = 0;
                    written += 1;
                }
            } else {
                for _ in 0..count {
                    residual[written] = reader.read_signed(raw_bits)?;
                    written += 1;
                }
            }
        } else {
            for _ in 0..count {
                let quotient = reader.read_unary()?;
                let remainder = if parameter == 0 {
                    0
                } else {
                    reader.read_bits(parameter)?
                };
                let folded = ((quotient as u64) << parameter) | remainder as u64;
                if folded > u32::MAX as u64 {
                    return Err(fatal(FlacError::CorruptFrame));
                }
                let folded = folded as u32;
                // Zig-zag: even -> non-negative, odd -> negative.
                let value = if folded & 1 == 0 {
                    (folded >> 1) as i64
                } else {
                    -(((folded >> 1) as i64) + 1)
                };
                if value < i32::MIN as i64 || value > i32::MAX as i64 {
                    return Err(fatal(FlacError::CorruptFrame));
                }
                residual[written] = value as i32;
                written += 1;
            }
        }
    }
    debug_assert_eq!(written, residual.len());
    Ok(())
}

/// Restores fixed-predictor samples in place. `samples[..order]` already holds
/// the warm-up samples and `samples[order..]` the residuals.
fn restore_fixed(samples: &mut [i32], order: usize) -> Result<(), Step> {
    for index in order..samples.len() {
        let predicted = match order {
            0 => 0i64,
            1 => samples[index - 1] as i64,
            2 => 2 * samples[index - 1] as i64 - samples[index - 2] as i64,
            3 => {
                3 * samples[index - 1] as i64 - 3 * samples[index - 2] as i64
                    + samples[index - 3] as i64
            }
            4 => {
                4 * samples[index - 1] as i64 - 6 * samples[index - 2] as i64
                    + 4 * samples[index - 3] as i64
                    - samples[index - 4] as i64
            }
            _ => return Err(fatal(FlacError::CorruptFrame)),
        };
        let restored = samples[index] as i64 + predicted;
        if restored < i32::MIN as i64 || restored > i32::MAX as i64 {
            return Err(fatal(FlacError::CorruptFrame));
        }
        samples[index] = restored as i32;
    }
    Ok(())
}

/// Restores LPC samples in place.
fn restore_lpc(
    samples: &mut [i32],
    order: usize,
    coefficients: &[i32; 32],
    shift: u32,
) -> Result<(), Step> {
    for index in order..samples.len() {
        let mut sum = 0i64;
        for tap in 0..order {
            sum += coefficients[tap] as i64 * samples[index - 1 - tap] as i64;
        }
        let restored = samples[index] as i64 + (sum >> shift);
        if restored < i32::MIN as i64 || restored > i32::MAX as i64 {
            return Err(fatal(FlacError::CorruptFrame));
        }
        samples[index] = restored as i32;
    }
    Ok(())
}

/// Undoes the stereo transform in place: afterwards channel 0 is left and
/// channel 1 is right.
fn apply_stereo(
    assignment: ChannelAssignment,
    channels: &mut [[i32; MAX_BLOCK_SIZE]; MAX_CHANNELS],
    block_size: usize,
) {
    match assignment {
        ChannelAssignment::Independent => {}
        ChannelAssignment::LeftSide => {
            for index in 0..block_size {
                let left = channels[0][index];
                let side = channels[1][index];
                channels[1][index] = left - side;
            }
        }
        ChannelAssignment::RightSide => {
            for index in 0..block_size {
                let side = channels[0][index];
                let right = channels[1][index];
                channels[0][index] = side + right;
            }
        }
        ChannelAssignment::MidSide => {
            for index in 0..block_size {
                let mid = channels[0][index];
                let side = channels[1][index];
                // The low bit of the side channel restores mid's lost bit.
                let mid = (mid << 1) | (side & 1);
                channels[0][index] = (mid + side) >> 1;
                channels[1][index] = (mid - side) >> 1;
            }
        }
    }
}

/// Maps a source sample onto the signed 16-bit output range.
fn to_i16(sample: i32, bits_per_sample: u8) -> i16 {
    let scaled = if bits_per_sample >= 16 {
        (sample as i64) >> (bits_per_sample - 16)
    } else {
        (sample as i64) << (16 - bits_per_sample)
    };
    scaled.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Marker for a read that ran past the buffered bytes.
#[derive(Debug)]
struct ReadError;

/// MSB-first bit reader over the buffered frame bytes.
struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read_bit(&mut self) -> Result<u32, ReadError> {
        if self.bit >= self.data.len() * 8 {
            return Err(ReadError);
        }
        let value = (self.data[self.bit >> 3] >> (7 - (self.bit & 7))) & 1;
        self.bit += 1;
        Ok(value as u32)
    }

    fn read_bits(&mut self, count: u32) -> Result<u32, ReadError> {
        if count == 0 {
            return Ok(0);
        }
        if count > 32 || self.bit + count as usize > self.data.len() * 8 {
            return Err(ReadError);
        }
        let mut value = 0u32;
        let mut left = count;
        while left > 0 {
            let byte = self.data[self.bit >> 3];
            let offset = (self.bit & 7) as u32;
            let available = 8 - offset;
            let take = available.min(left);
            let shift = available - take;
            let mask = (1u16 << take) - 1;
            let part = ((byte >> shift) as u16 & mask) as u32;
            value = (value << take) | part;
            self.bit += take as usize;
            left -= take;
        }
        Ok(value)
    }

    fn read_signed(&mut self, count: u32) -> Result<i32, ReadError> {
        let raw = self.read_bits(count)?;
        if count == 0 {
            return Ok(0);
        }
        if count == 32 {
            return Ok(raw as i32);
        }
        let shift = 32 - count;
        Ok(((raw << shift) as i32) >> shift)
    }

    /// Counts zero bits up to and including the next one bit.
    fn read_unary(&mut self) -> Result<u32, ReadError> {
        let mut zeros = 0u32;
        loop {
            if self.read_bit()? != 0 {
                return Ok(zeros);
            }
            zeros = zeros.checked_add(1).ok_or(ReadError)?;
        }
    }

    fn align(&mut self) {
        self.bit = (self.bit + 7) & !7;
    }

    fn byte_position(&self) -> usize {
        self.bit / 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small MSB-first writer used to hand-build subframe bitstreams.
    struct BitWriter {
        data: [u8; 32],
        bit: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                data: [0; 32],
                bit: 0,
            }
        }

        fn write(&mut self, value: u32, bits: u32) {
            for index in (0..bits).rev() {
                if (value >> index) & 1 == 1 {
                    self.data[self.bit >> 3] |= 1 << (7 - (self.bit & 7));
                }
                self.bit += 1;
            }
        }

        fn bytes(&self) -> &[u8] {
            &self.data[..(self.bit + 7) / 8]
        }
    }

    #[test]
    fn verbatim_subframes_with_wasted_bits_are_restored() {
        let mut writer = BitWriter::new();
        writer.write(0, 1); // zero padding bit
        writer.write(1, 6); // verbatim
        writer.write(1, 1); // wasted-bits flag
        writer.write(1, 2); // unary "01" -> two wasted bits
        // Stored values are the samples shifted right by two.
        for stored in [100i32, -100, 300, -300] {
            writer.write((stored as u32) & 0x3fff, 14);
        }
        let mut samples = [0i32; 4];
        let mut reader = BitReader::new(writer.bytes());
        decode_subframe(&mut reader, 16, 4, &mut samples).unwrap();
        assert_eq!(samples, [400, -400, 1_200, -1_200]);
    }

    #[test]
    fn fixed_predictors_and_rice_escape_codes_are_decoded() {
        let mut writer = BitWriter::new();
        writer.write(0, 1); // zero padding bit
        writer.write(9, 6); // fixed predictor, order 1
        writer.write(0, 1); // no wasted bits
        writer.write(10, 16); // warm-up sample
        writer.write(0, 2); // residual method: Rice with 4-bit parameters
        writer.write(0, 4); // one partition
        writer.write(0xf, 4); // escape code
        writer.write(5, 5); // five raw bits per residual
        for residual in [1i32, -1, 2] {
            writer.write((residual as u32) & 0x1f, 5);
        }
        let mut samples = [0i32; 4];
        let mut reader = BitReader::new(writer.bytes());
        decode_subframe(&mut reader, 16, 4, &mut samples).unwrap();
        // 10, 10+1, 11-1, 10+2
        assert_eq!(samples, [10, 11, 10, 12]);
    }

    #[test]
    fn crc_checks_known_vectors() {
        // FLAC's CRC-8 (poly 0x07) and CRC-16 (poly 0x8005), both MSB-first.
        assert_eq!(crc8(b"123456789"), 0xf4);
        assert_eq!(crc16(b"123456789"), 0xfee8);
    }

    #[test]
    fn the_bit_reader_is_msb_first_and_sign_extends() {
        let data = [0b1010_0110, 0xff, 0x00];
        let mut reader = BitReader::new(&data);
        assert_eq!(reader.read_bits(3).unwrap(), 0b101);
        assert_eq!(reader.read_bits(5).unwrap(), 0b0_0110);
        assert_eq!(reader.read_signed(4).unwrap(), -1);
        assert_eq!(reader.read_bits(4).unwrap(), 0xf);
        assert_eq!(reader.read_bits(8).unwrap(), 0);
        assert!(reader.read_bit().is_err());

        let mut reader = BitReader::new(&[0b0000_0010, 0b0000_0001]);
        assert_eq!(reader.read_unary().unwrap(), 6);
        // One zero finishes the first byte, seven more fill the second.
        assert_eq!(reader.read_unary().unwrap(), 8);
    }

    #[test]
    fn scaling_keeps_sixteen_bits_and_truncates_deeper_samples() {
        assert_eq!(to_i16(1234, 16), 1234);
        assert_eq!(to_i16(0x7f_ffff, 24), 32767);
        assert_eq!(to_i16(-0x80_0000, 24), -32768);
        assert_eq!(to_i16(-1, 24), -1); // arithmetic shift, not rounding toward zero
        assert_eq!(to_i16(0x80, 12), 0x800);
    }

    #[test]
    fn stereo_decorrelation_reconstructs_left_and_right() {
        let mut channels = [[0i32; MAX_BLOCK_SIZE]; MAX_CHANNELS];
        // left = 100, right = 70  ->  mid = 85, side = 30
        for (left, right) in [(100, 70), (-101, 70), (0, 0), (32767, -32768)] {
            let mid = (left + right) >> 1;
            let side = left - right;
            channels[0][0] = mid;
            channels[1][0] = side;
            apply_stereo(ChannelAssignment::MidSide, &mut channels, 1);
            assert_eq!((channels[0][0], channels[1][0]), (left, right));

            channels[0][0] = left;
            channels[1][0] = side;
            apply_stereo(ChannelAssignment::LeftSide, &mut channels, 1);
            assert_eq!((channels[0][0], channels[1][0]), (left, right));

            channels[0][0] = side;
            channels[1][0] = right;
            apply_stereo(ChannelAssignment::RightSide, &mut channels, 1);
            assert_eq!((channels[0][0], channels[1][0]), (left, right));
        }
    }

    #[test]
    fn streaminfo_validation_refuses_what_it_cannot_play() {
        let mut body = [0u8; 34];
        body[0..2].copy_from_slice(&4096u16.to_be_bytes());
        body[2..4].copy_from_slice(&4096u16.to_be_bytes());
        let pack = |rate: u32, channels: u8, bits: u8| {
            ((rate as u64) << 44) | (((channels - 1) as u64) << 41) | (((bits - 1) as u64) << 36)
        };
        let mut ok = body;
        ok[10..18].copy_from_slice(&pack(44_100, 2, 16).to_be_bytes());
        assert_eq!(parse_stream_info(&ok).unwrap().channels, 2);

        let mut too_wide = ok;
        too_wide[10..18].copy_from_slice(&pack(44_100, 2, 32).to_be_bytes());
        assert!(matches!(
            parse_stream_info(&too_wide),
            Err(Step::Fatal(FlacError::UnsupportedSampleSize(32)))
        ));

        let mut too_many = ok;
        too_many[10..18].copy_from_slice(&pack(44_100, 3, 16).to_be_bytes());
        assert!(matches!(
            parse_stream_info(&too_many),
            Err(Step::Fatal(FlacError::UnsupportedChannelCount(3)))
        ));

        let mut bad_rate = ok;
        bad_rate[10..18].copy_from_slice(&pack(0, 2, 16).to_be_bytes());
        assert!(matches!(
            parse_stream_info(&bad_rate),
            Err(Step::Fatal(FlacError::UnsupportedSampleRate(0)))
        ));
    }
}
