//! Streaming MPEG-1 Audio Layer III (MP3) decoder.
//!
//! The decoder is written for the music player in InterOS: the caller reads the
//! file sequentially in bounded chunks (2 KiB through the capability gated
//! filesystem), pushes those bytes with [`Mp3Decoder::push`] and drains ready
//! frames with [`Mp3Decoder::next_frame`]. Nothing is allocated: all bit
//! reservoir, overlap and synthesis state lives in fixed size arrays inside
//! [`Mp3Decoder`]. The caller supplies the PCM buffer, so the decoder also
//! never needs an output queue.
//!
//! ```text
//! let mut decoder = Mp3Decoder::new();
//! let mut pcm = [0i16; MAX_FRAME_SAMPLES * 2];
//! loop {
//!     let n = fs.read(&mut chunk)?;
//!     if n == 0 { decoder.finish(); }
//!     else { decoder.push(&chunk[..n])?; }
//!     while let Some(info) = decoder.next_frame(&mut pcm)? {
//!         // info.channels interleaved i16 samples at info.sample_rate
//!     }
//!     if n == 0 { break; }
//! }
//! ```
//!
//! Decoded frames are interleaved signed 16-bit PCM at the stream's own sample
//! rate, exactly [`MAX_FRAME_SAMPLES`] samples per channel. A frame that is not
//! complete yet makes `next_frame` return `Ok(None)`; call it again after the
//! next push. [`Mp3Decoder::finish`] declares end of input so an incomplete
//! trailing frame is reported as [`Mp3Error::Truncated`] instead of waiting
//! forever.
//!
//! # Supported
//!
//! * MPEG-1 Layer III at 32, 44.1 and 48 kHz, all bitrates of the standard.
//! * Stereo, joint stereo (middle/side and intensity), dual channel and mono.
//! * The Layer III bit reservoir (`main_data_begin`), scalefactor selection
//!   information, short/mixed/long blocks, Huffman decoding including count1
//!   and linbits tables, requantisation, alias reduction, IMDCT, overlap-add
//!   and the polyphase synthesis filterbank.
//! * A bounded ID3v2 tag at the start of the stream is skipped.
//!
//! # Refused with a typed error
//!
//! * MPEG-2 and MPEG-2.5 Layer III (LSF) -> [`Mp3Error::UnsupportedVersion`].
//!   Their 576-sample granules need the LSF scalefactor and table set; this
//!   decoder deliberately does not pretend to decode them.
//! * Layer I and Layer II -> [`Mp3Error::UnsupportedLayer`].
//! * Free format streams (no bitrate index) -> [`Mp3Error::FreeFormat`].
//! * Corrupt headers, corrupt main data and reservoirs that reference more
//!   than the 511 bytes the standard allows -> [`Mp3Error::InvalidFrame`] or
//!   [`Mp3Error::HuffmanError`].
//! * Truncated input at end of stream -> [`Mp3Error::Truncated`].
//! * Bytes that never form a frame sync -> [`Mp3Error::SyncLost`].
//!
//! # Limitations
//!
//! * The CRC of protected frames is skipped, not verified (the bit reservoir
//!   and audio data are still consumed correctly).
//! * An ID3v1 tag at the *end* of a file is not skipped; it produces
//!   [`Mp3Error::SyncLost`] after the last audio frame.
//! * Seeking is not supported by design.

use core::cmp;

/// Samples per channel in one decoded MPEG-1 Layer III frame (2 granules of
/// 576 samples).
pub const MAX_FRAME_SAMPLES: usize = 1152;

/// Maximum number of channels the decoder produces.
pub const MAX_CHANNELS: usize = 2;

/// Reception buffer. The player pushes at most 2 KiB at a time, and at most a
/// little less than one frame is left over between pushes.
const INPUT_CAPACITY: usize = 4096;

/// Bit reservoir: 511 bytes of history plus the largest MPEG-1 Layer III frame
/// payload (1441 - 4 - 32 bytes).
const MAIN_CAPACITY: usize = 2048;

/// Largest possible MPEG-1 Layer III frame at 320 kbps / 32 kHz plus padding.
const MAX_FRAME_LEN: usize = 1441;

/// Upper bound for an ID3v2 tag we are willing to skip.
const MAX_ID3_BYTES: usize = 1024 * 1024;

/// Information about one decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Bitrate of this frame in kbit/s.
    pub bitrate_kbps: u16,
    /// Sampling rate of the stream in Hz (32000, 44100 or 48000).
    pub sample_rate: u32,
    /// 1 for mono, 2 for any stereo mode.
    pub channels: u8,
    /// Samples produced per channel by this call (always 1152).
    pub samples_per_channel: usize,
    /// `main_data_begin` of this frame: how many bytes of the preceding
    /// frames' main data the frame borrowed from the bit reservoir.
    pub main_data_begin: u16,
}

impl FrameInfo {
    /// Total interleaved samples produced by this frame.
    pub const fn samples(&self) -> usize {
        self.samples_per_channel * self.channels as usize
    }
}

/// Errors reported by [`Mp3Decoder`]. Every condition is distinct so the
/// player can tell "skip this file" from "read more bytes".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mp3Error {
    /// The reception buffer is full. The caller must drain frames first.
    BufferFull,
    /// Input ended (or contained bytes) without a valid frame sync.
    SyncLost,
    /// Input ended in the middle of a frame, side info or ID3 tag.
    Truncated,
    /// The ID3v2 tag at the start is larger than 1 MiB.
    Id3TooLarge,
    /// The header uses the free format bitrate index, which is not supported.
    FreeFormat,
    /// The header selects Layer I or Layer II.
    UnsupportedLayer,
    /// The header selects MPEG-2 or MPEG-2.5 (low sampling frequency).
    UnsupportedVersion,
    /// The header or side info is structurally invalid.
    InvalidFrame,
    /// The frame borrows more than 511 bytes from the bit reservoir.
    ReservoirUnderflow,
    /// The Huffman data or a linbits escape runs past the allocated bits.
    HuffmanError,
    /// The caller's PCM buffer is smaller than 1152 * channels samples.
    OutputTooSmall,
}

/// A streaming MPEG-1 Layer III decoder.
///
/// Create with [`Mp3Decoder::new`], feed bytes with [`Mp3Decoder::push`] and
/// pull PCM with [`Mp3Decoder::next_frame`]. All memory is internal and fixed.
pub struct Mp3Decoder {
    input: [u8; INPUT_CAPACITY],
    input_len: usize,
    ended: bool,
    at_start: bool,
    id3_remaining: usize,

    main_buf: [u8; MAIN_CAPACITY],
    main_history: usize,

    spec: [[f32; 576]; MAX_CHANNELS],
    overlap: [[f32; 18]; MAX_CHANNELS * 32],
    v_buf: [[f32; 1024]; MAX_CHANNELS],

    stream_format: Option<(u32, u8)>,
}

impl Mp3Decoder {
    /// Creates a decoder with cleared reservoir, overlap and synthesis state.
    pub const fn new() -> Self {
        Self {
            input: [0; INPUT_CAPACITY],
            input_len: 0,
            ended: false,
            at_start: true,
            id3_remaining: 0,
            main_buf: [0; MAIN_CAPACITY],
            main_history: 0,
            spec: [[0.0; 576]; MAX_CHANNELS],
            overlap: [[0.0; 18]; MAX_CHANNELS * 32],
            v_buf: [[0.0; 1024]; MAX_CHANNELS],
            stream_format: None,
        }
    }

    /// Number of bytes currently available in the input buffer.
    pub const fn available(&self) -> usize {
        INPUT_CAPACITY - self.input_len
    }

    /// Adds bytes to the input buffer. Returns [`Mp3Error::BufferFull`] if the
    /// caller pushes more than 4 KiB without draining frames.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), Mp3Error> {
        if bytes.len() > INPUT_CAPACITY - self.input_len {
            return Err(Mp3Error::BufferFull);
        }
        let end = self.input_len + bytes.len();
        self.input[self.input_len..end].copy_from_slice(bytes);
        self.input_len = end;
        self.ended = false;
        Ok(())
    }

    /// Declares that no more bytes will be pushed. A frame that is still
    /// incomplete is reported as [`Mp3Error::Truncated`] by `next_frame`.
    pub fn finish(&mut self) {
        self.ended = true;
    }

    /// Decodes the next complete frame.
    ///
    /// Returns `Ok(None)` when more bytes are needed, `Ok(Some(info))` after
    /// writing `info.samples_per_channel * info.channels` interleaved samples
    /// to `out`, and a typed error for refused or corrupt input.
    pub fn next_frame(&mut self, out: &mut [i16]) -> Result<Option<FrameInfo>, Mp3Error> {
        loop {
            // 1. Skip a bounded ID3v2 tag at the very start.
            if self.id3_remaining > 0 {
                let take = cmp::min(self.id3_remaining, self.input_len);
                self.consume(take);
                self.id3_remaining -= take;
                if self.id3_remaining > 0 {
                    if self.ended {
                        return Err(Mp3Error::Truncated);
                    }
                    return Ok(None);
                }
            }
            if self.at_start {
                if self.input_len == 0 {
                    return Ok(None);
                }
                if self.input_len < 3 {
                    if !self.ended {
                        return Ok(None);
                    }
                    return Err(Mp3Error::SyncLost);
                }
                if self.input[0] == b'I' && self.input[1] == b'D' && self.input[2] == b'3' {
                    if self.input_len < 10 {
                        if !self.ended {
                            return Ok(None);
                        }
                        return Err(Mp3Error::Truncated);
                    }
                    let mut header = [0u8; 10];
                    header.copy_from_slice(&self.input[..10]);
                    let total = id3v2_len(&header)?;
                    self.consume(10);
                    self.id3_remaining = total - 10;
                    self.at_start = false;
                    continue;
                }
                self.at_start = false;
            }

            // 2. Find the next frame sync. Anything before it is junk.
            if self.input_len == 0 {
                return Ok(None);
            }
            if self.input[0] != 0xFF {
                let mut next = None;
                for (i, &byte) in self.input[..self.input_len].iter().enumerate() {
                    if byte == 0xFF {
                        next = Some(i);
                        break;
                    }
                }
                match next {
                    Some(i) => self.consume(i),
                    None => {
                        self.consume(self.input_len);
                        if self.ended {
                            return Err(Mp3Error::SyncLost);
                        }
                        return Ok(None);
                    }
                }
                continue;
            }
            if self.input_len < 4 {
                if !self.ended {
                    return Ok(None);
                }
                return Err(Mp3Error::Truncated);
            }

            // 3. Parse the header and wait for the complete frame.
            let header = parse_header(&self.input[..4])?;
            if self.input_len < header.frame_len {
                if !self.ended {
                    return Ok(None);
                }
                return Err(Mp3Error::Truncated);
            }
            let needed = MAX_FRAME_SAMPLES * header.channels as usize;
            if out.len() < needed {
                return Err(Mp3Error::OutputTooSmall);
            }

            // Copy the frame out of the reception buffer so the rest of the
            // frame can borrow `self` mutably.
            let mut frame = [0u8; MAX_FRAME_LEN];
            frame[..header.frame_len].copy_from_slice(&self.input[..header.frame_len]);
            self.consume(header.frame_len);

            if let Some((rate, channels)) = self.stream_format {
                if rate != header.sample_rate || channels != header.channels {
                    self.clear_pcm_state();
                }
            }
            self.stream_format = Some((header.sample_rate, header.channels));

            if let Some(info) = self.decode_frame(&header, &frame[..header.frame_len], out)? {
                return Ok(Some(info));
            }
            // Frame skipped: its granule started before the reservoir was
            // primed. Keep looking at the following frame.
        }
    }

    /// Forgets all input and decoder state, as if freshly constructed.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn consume(&mut self, count: usize) {
        debug_assert!(count <= self.input_len);
        self.input.copy_within(count..self.input_len, 0);
        self.input_len -= count;
    }

    fn clear_pcm_state(&mut self) {
        self.overlap = [[0.0; 18]; MAX_CHANNELS * 32];
        self.v_buf = [[0.0; 1024]; MAX_CHANNELS];
    }

    /// Decodes one complete frame. `Ok(None)` means the frame's main data
    /// begins in frames that are not in the reservoir yet, so the frame was
    /// consumed and must be skipped (normal at the start of a mid-stream
    /// capture).
    fn decode_frame(
        &mut self,
        header: &Header,
        frame: &[u8],
        out: &mut [i16],
    ) -> Result<Option<FrameInfo>, Mp3Error> {
        let channels = header.channels as usize;
        let side_start = 4 + if header.crc { 2 } else { 0 };
        let side_len = header.side_len;
        if frame.len() < side_start + side_len {
            return Err(Mp3Error::InvalidFrame);
        }
        let main_start = side_start + side_len;
        let main_len = frame.len() - main_start;

        let mut side_reader = BitReader::new(&frame[side_start..main_start]);
        let mut side = SideInfo::parse(&mut side_reader, channels, header.version)?;

        if self.main_history + main_len > MAIN_CAPACITY {
            return Err(Mp3Error::InvalidFrame);
        }
        self.main_buf[self.main_history..self.main_history + main_len]
            .copy_from_slice(&frame[main_start..]);
        let total_main = self.main_history + main_len;
        let main_data_begin = side.main_data_begin as usize;
        if main_data_begin > 511 {
            return Err(Mp3Error::ReservoirUnderflow);
        }
        if main_data_begin > self.main_history {
            // The bytes this frame's first granule starts in are not available.
            // Keep the frame's own payload for the frames that follow.
            let keep = cmp::min(total_main, 511);
            self.main_buf.copy_within(total_main - keep..total_main, 0);
            self.main_history = keep;
            return Ok(None);
        }

        let window_start = self.main_history - main_data_begin;
        let sfreq = header.sample_rate_index;
        let mut scalefacs = Scalefacs::zero();
        {
            let main = &self.main_buf[window_start..total_main];
            let mut bits = BitReader::new(main);
            // The two granules share one bit stream, but `self.spec` holds only
            // one granule: decode a granule and run it through the audio path
            // before the next granule overwrites the coefficients.
            let granules_count = if header.version == 3 { 2 } else { 1 };
            let samples_per_channel = if header.version == 3 { MAX_FRAME_SAMPLES } else { 576 };
            if out.len() < samples_per_channel * channels {
                return Err(Mp3Error::OutputTooSmall);
            }
            for gr in 0..granules_count {
                for ch in 0..channels {
                    let gi = side.gr[gr][ch];
                    if gi.part2_3_length == 0 {
                        self.spec[ch] = [0.0; 576];
                        side.count1[gr][ch] = 0;
                        continue;
                    }
                    let part_start = bits.position();
                    read_scalefactors(&mut bits, &mut scalefacs, &side, gr, ch, &gi, header.version)?;
                    read_huffman(
                        &mut bits,
                        &mut side,
                        &mut self.spec[ch],
                        gr,
                        ch,
                        part_start,
                        header.sample_rate_index,
                    )?;
                }
                for ch in 0..channels {
                    let gi = side.gr[gr][ch];
                    requantize(&mut self.spec[ch], &scalefacs, &side, gr, ch, &gi, sfreq);
                    reorder(&mut self.spec[ch], sfreq, &gi);
                }
                if channels == 2 {
                    stereo_process(
                        &mut self.spec,
                        &scalefacs,
                        &side,
                        gr,
                        header.mode_extension,
                        sfreq,
                    );
                }
                for ch in 0..channels {
                    let gi = side.gr[gr][ch];
                    antialias(&mut self.spec[ch], &gi);
                    hybrid_synthesis(&mut self.spec[ch], &mut self.overlap, ch, &gi);
                    frequency_inversion(&mut self.spec[ch]);
                    subband_synthesis(
                        &self.spec[ch],
                        &mut self.v_buf[ch],
                        out,
                        gr * 576,
                        channels,
                        ch,
                    );
                }
            }
        }
        // Keep at most the 511 bytes an MPEG-1 frame may reference.
        let keep = cmp::min(total_main, 511);
        self.main_buf.copy_within(total_main - keep..total_main, 0);
        self.main_history = keep;

        let samples_per_channel = if header.version == 3 { MAX_FRAME_SAMPLES } else { 576 };
        Ok(Some(FrameInfo {
            bitrate_kbps: header.bitrate_kbps,
            sample_rate: header.sample_rate,
            channels: header.channels,
            samples_per_channel,
            main_data_begin: side.main_data_begin,
        }))
    }
}

impl Default for Mp3Decoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Header and side information
// ---------------------------------------------------------------------------

const BITRATE_KBPS: [u16; 15] =
    [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const SAMPLE_RATES: [u32; 3] = [44100, 48000, 32000];

const BITRATE_KBPS_MPEG2: [u16; 14] = [
    8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160,
];
const SAMPLE_RATES_MPEG2: [u32; 3] = [22_050, 24_000, 16_000];

#[derive(Clone, Copy)]
struct Header {
    version: u8,
    bitrate_kbps: u16,
    sample_rate: u32,
    sample_rate_index: usize,
    channels: u8,
    mode_extension: u8,
    crc: bool,
    frame_len: usize,
    side_len: usize,
}

/// Parses the four byte header at `bytes[0..4]`.
fn parse_header(bytes: &[u8]) -> Result<Header, Mp3Error> {
    if bytes.len() < 4 {
        return Err(Mp3Error::Truncated);
    }
    if bytes[0] != 0xFF || (bytes[1] & 0xE0) != 0xE0 {
        return Err(Mp3Error::SyncLost);
    }
    let version = (bytes[1] >> 3) & 0x03;
    let layer = (bytes[1] >> 1) & 0x03;
    if version == 1 {
        return Err(Mp3Error::InvalidFrame);
    }
    if version == 0 {
        return Err(Mp3Error::UnsupportedVersion);
    }
    if layer != 1 {
        return Err(Mp3Error::UnsupportedLayer);
    }
    let bitrate_index = ((bytes[2] >> 4) & 0x0F) as usize;
    if bitrate_index == 0 {
        return Err(Mp3Error::FreeFormat);
    }
    if bitrate_index == 15 {
        return Err(Mp3Error::InvalidFrame);
    }
    let sample_rate_index = ((bytes[2] >> 2) & 0x03) as usize;
    if sample_rate_index == 3 {
        return Err(Mp3Error::InvalidFrame);
    }
    let padding = ((bytes[2] >> 1) & 0x01) as usize;
    let mode = (bytes[3] >> 6) & 0x03;
    let mode_extension = (bytes[3] >> 4) & 0x03;
    let crc = (bytes[1] & 0x01) == 0;
    let bitrate_kbps = if version == 3 {
        BITRATE_KBPS[bitrate_index - 1]
    } else {
        BITRATE_KBPS_MPEG2[bitrate_index - 1]
    };
    let sample_rate = if version == 3 {
        SAMPLE_RATES[sample_rate_index]
    } else {
        SAMPLE_RATES_MPEG2[sample_rate_index]
    };
    let coef = if version == 3 { 144 } else { 72 };
    let frame_len =
        coef * bitrate_kbps as usize * 1000 / sample_rate as usize + padding;
    let header_len = 4 + if crc { 2 } else { 0 };
    let side_len = if version == 3 {
        if mode == 3 { 17 } else { 32 }
    } else {
        if mode == 3 { 9 } else { 17 }
    };
    if frame_len <= header_len + side_len {
        return Err(Mp3Error::InvalidFrame);
    }
    Ok(Header {
        version,
        bitrate_kbps,
        sample_rate,
        sample_rate_index,
        channels: if mode == 3 { 1 } else { 2 },
        mode_extension,
        crc,
        frame_len,
        side_len,
    })
}

/// Returns the total number of bytes occupied by an ID3v2 tag whose ten byte
/// header is `header` (including the header and an optional footer).
fn id3v2_len(header: &[u8; 10]) -> Result<usize, Mp3Error> {
    let size = ((header[6] as usize & 0x7F) << 21)
        | ((header[7] as usize & 0x7F) << 14)
        | ((header[8] as usize & 0x7F) << 7)
        | (header[9] as usize & 0x7F);
    if size > MAX_ID3_BYTES {
        return Err(Mp3Error::Id3TooLarge);
    }
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    Ok(10 + size + footer)
}

#[derive(Clone, Copy)]
struct GranuleInfo {
    part2_3_length: u16,
    big_values: u16,
    global_gain: u8,
    scalefac_compress: u16,
    window_switching: bool,
    block_type: u8,
    mixed_block: bool,
    table_select: [u8; 3],
    subblock_gain: [u8; 3],
    region0_count: u8,
    region1_count: u8,
    preflag: bool,
    scalefac_scale: bool,
    count1table_select: bool,
}

impl GranuleInfo {
    const ZERO: Self = Self {
        part2_3_length: 0,
        big_values: 0,
        global_gain: 0,
        scalefac_compress: 0,
        window_switching: false,
        block_type: 0,
        mixed_block: false,
        table_select: [0; 3],
        subblock_gain: [0; 3],
        region0_count: 0,
        region1_count: 0,
        preflag: false,
        scalefac_scale: false,
        count1table_select: false,
    };
}

struct SideInfo {
    main_data_begin: u16,
    scfsi: [[u8; 4]; 2],
    gr: [[GranuleInfo; 2]; 2],
    /// First frequency line that belongs to the zero (not decoded) region.
    count1: [[u16; 2]; 2],
}

impl SideInfo {
    fn parse(bits: &mut BitReader<'_>, channels: usize, version: u8) -> Result<Self, Mp3Error> {
        let mut side = SideInfo {
            main_data_begin: 0,
            scfsi: [[0; 4]; 2],
            gr: [[GranuleInfo::ZERO; 2]; 2],
            count1: [[0; 2]; 2],
        };
        if version == 2 {
            side.main_data_begin = bits.read_bits(8).ok_or(Mp3Error::InvalidFrame)? as u16;
            let private_bits = if channels == 1 { 1u32 } else { 2u32 };
            let _ = bits.read_bits(private_bits).ok_or(Mp3Error::InvalidFrame)?;
            for ch in 0..channels {
                let mut gi = GranuleInfo::ZERO;
                gi.part2_3_length =
                    bits.read_bits(12).ok_or(Mp3Error::InvalidFrame)? as u16;
                gi.big_values = bits.read_bits(9).ok_or(Mp3Error::InvalidFrame)? as u16;
                if gi.big_values > 288 {
                    return Err(Mp3Error::InvalidFrame);
                }
                gi.global_gain =
                    bits.read_bits(8).ok_or(Mp3Error::InvalidFrame)? as u8;
                gi.scalefac_compress =
                    bits.read_bits(9).ok_or(Mp3Error::InvalidFrame)? as u16;
                gi.window_switching =
                    bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                if gi.window_switching {
                    gi.block_type =
                        bits.read_bits(2).ok_or(Mp3Error::InvalidFrame)? as u8;
                    gi.mixed_block =
                        bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                    gi.table_select[0] =
                        bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                    gi.table_select[1] =
                        bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                    for window in 0..3 {
                        gi.subblock_gain[window] =
                            bits.read_bits(3).ok_or(Mp3Error::InvalidFrame)? as u8;
                    }
                    gi.region0_count =
                        if gi.block_type == 2 && !gi.mixed_block { 8 } else { 7 };
                    gi.region1_count = 20 - gi.region0_count;
                } else {
                    for region in 0..3 {
                        gi.table_select[region] =
                            bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                    }
                    gi.region0_count =
                        bits.read_bits(4).ok_or(Mp3Error::InvalidFrame)? as u8;
                    gi.region1_count =
                        bits.read_bits(3).ok_or(Mp3Error::InvalidFrame)? as u8;
                }
                gi.preflag = false;
                gi.scalefac_scale =
                    bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                gi.count1table_select =
                    bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                side.gr[0][ch] = gi;
            }
        } else {
            side.main_data_begin = bits.read_bits(9).ok_or(Mp3Error::InvalidFrame)? as u16;
            let private_bits = if channels == 1 { 5u32 } else { 3u32 };
            let _ = bits.read_bits(private_bits).ok_or(Mp3Error::InvalidFrame)?;
            for ch in 0..channels {
                for band in 0..4 {
                    side.scfsi[ch][band] =
                        bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? as u8;
                }
            }
            for gr in 0..2 {
                for ch in 0..channels {
                    let mut gi = GranuleInfo::ZERO;
                    gi.part2_3_length =
                        bits.read_bits(12).ok_or(Mp3Error::InvalidFrame)? as u16;
                    gi.big_values = bits.read_bits(9).ok_or(Mp3Error::InvalidFrame)? as u16;
                    if gi.big_values > 288 {
                        return Err(Mp3Error::InvalidFrame);
                    }
                    gi.global_gain =
                        bits.read_bits(8).ok_or(Mp3Error::InvalidFrame)? as u8;
                    gi.scalefac_compress =
                        bits.read_bits(4).ok_or(Mp3Error::InvalidFrame)? as u16;
                    gi.window_switching =
                        bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                    if gi.window_switching {
                        gi.block_type =
                            bits.read_bits(2).ok_or(Mp3Error::InvalidFrame)? as u8;
                        gi.mixed_block =
                            bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                        gi.table_select[0] =
                            bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                        gi.table_select[1] =
                            bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                        for window in 0..3 {
                            gi.subblock_gain[window] =
                                bits.read_bits(3).ok_or(Mp3Error::InvalidFrame)? as u8;
                        }
                        gi.region0_count =
                            if gi.block_type == 2 && !gi.mixed_block { 8 } else { 7 };
                        gi.region1_count = 20 - gi.region0_count;
                    } else {
                        for region in 0..3 {
                            gi.table_select[region] =
                                bits.read_bits(5).ok_or(Mp3Error::InvalidFrame)? as u8;
                        }
                        gi.region0_count =
                            bits.read_bits(4).ok_or(Mp3Error::InvalidFrame)? as u8;
                        gi.region1_count =
                            bits.read_bits(3).ok_or(Mp3Error::InvalidFrame)? as u8;
                    }
                    gi.preflag = bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                    gi.scalefac_scale =
                        bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                    gi.count1table_select =
                        bits.read_bits(1).ok_or(Mp3Error::InvalidFrame)? != 0;
                    side.gr[gr][ch] = gi;
                }
            }
        }
        Ok(side)
    }
}

// ---------------------------------------------------------------------------
// Bit reader and Huffman decoding
// ---------------------------------------------------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn position(&self) -> usize {
        self.bit
    }

    fn set_position(&mut self, bit: usize) {
        self.bit = bit;
    }

    fn total_bits(&self) -> usize {
        self.data.len() * 8
    }

    fn read_bit(&mut self) -> Option<u32> {
        let byte = self.bit >> 3;
        if byte >= self.data.len() {
            return None;
        }
        let value = (self.data[byte] >> (7 - (self.bit & 7))) & 1;
        self.bit += 1;
        Some(value as u32)
    }

    fn read_bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()?;
        }
        Some(value)
    }
}

const SCALEFAC_SIZES: [[u8; 2]; 16] = [
    [0, 0],
    [0, 1],
    [0, 2],
    [0, 3],
    [3, 0],
    [1, 1],
    [1, 2],
    [1, 3],
    [2, 1],
    [2, 2],
    [2, 3],
    [3, 1],
    [3, 2],
    [3, 3],
    [4, 2],
    [4, 3],
];

/// Decodes one Huffman word. Returns `(x, y, v, w)`; `v`/`w` are only used by
/// the four-value count1 tables (32 and 33).
fn huffman_decode(
    bits: &mut BitReader<'_>,
    table: usize,
) -> Result<(i32, i32, i32, i32), Mp3Error> {
    if table >= 34 {
        return Err(Mp3Error::HuffmanError);
    }
    let len = HUFF_LEN[table] as usize;
    if len == 0 {
        return Ok((0, 0, 0, 0));
    }
    let base = HUFF_OFFSET[table] as usize;
    let mut point = 0usize;
    let leaf;
    loop {
        if point >= len {
            return Err(Mp3Error::HuffmanError);
        }
        let node = HUFF_TREE[base + point];
        if node & 0xFF00 == 0 {
            leaf = node;
            break;
        }
        let branch = bits.read_bit().ok_or(Mp3Error::HuffmanError)?;
        let offset = if branch == 1 {
            let mut off = (HUFF_TREE[base + point] & 0xFF) as usize;
            while off >= 250 {
                point += off;
                if point >= len {
                    return Err(Mp3Error::HuffmanError);
                }
                off = (HUFF_TREE[base + point] & 0xFF) as usize;
            }
            off
        } else {
            let mut off = (HUFF_TREE[base + point] >> 8) as usize;
            while off >= 250 {
                point += off;
                if point >= len {
                    return Err(Mp3Error::HuffmanError);
                }
                off = (HUFF_TREE[base + point] >> 8) as usize;
            }
            off
        };
        point += offset;
    }

    let mut x = ((leaf >> 4) & 0x0F) as i32;
    let mut y = (leaf & 0x0F) as i32;
    if table > 31 {
        let mut v = (y >> 3) & 1;
        let mut w = (y >> 2) & 1;
        let mut xx = (y >> 1) & 1;
        let mut yy = y & 1;
        if v != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            v = -v;
        }
        if w != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            w = -w;
        }
        if xx != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            xx = -xx;
        }
        if yy != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            yy = -yy;
        }
        Ok((xx, yy, v, w))
    } else {
        let linbits = HUFF_LINBITS[table] as u32;
        if linbits > 0 && x == 15 {
            x += bits.read_bits(linbits).ok_or(Mp3Error::HuffmanError)? as i32;
        }
        if x != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            x = -x;
        }
        if linbits > 0 && y == 15 {
            y += bits.read_bits(linbits).ok_or(Mp3Error::HuffmanError)? as i32;
        }
        if y != 0 && bits.read_bit().ok_or(Mp3Error::HuffmanError)? == 1 {
            y = -y;
        }
        Ok((x, y, 0, 0))
    }
}

// ---------------------------------------------------------------------------
// Scalefactors, requantisation and reordering
// ---------------------------------------------------------------------------

struct Scalefacs {
    /// `[granule][channel][scalefactor band]`
    long: [[[u8; 23]; 2]; 2],
    /// `[granule][channel][scalefactor band][window]`
    short: [[[[u8; 3]; 13]; 2]; 2],
}

impl Scalefacs {
    fn zero() -> Self {
        Self {
            long: [[[0; 23]; 2]; 2],
            short: [[[[0; 3]; 13]; 2]; 2],
        }
    }
}

fn read_scalefactors(
    bits: &mut BitReader<'_>,
    scal: &mut Scalefacs,
    side: &SideInfo,
    gr: usize,
    ch: usize,
    gi: &GranuleInfo,
    version: u8,
) -> Result<(), Mp3Error> {
    if version == 2 {
        let sfc = gi.scalefac_compress as usize;
        let (slen1, slen2, slen3, slen4) = if sfc < 400 {
            let n = sfc >> 4;
            let slen1 = (n / 5) as u32;
            let slen2 = (n % 5) as u32;
            let slen3 = ((sfc >> 2) & 0x03) as u32;
            let slen4 = (sfc & 0x03) as u32;
            (slen1, slen2, slen3, slen4)
        } else if sfc < 500 {
            let n = (sfc - 400) >> 2;
            let slen1 = (n / 5) as u32;
            let slen2 = (n % 5) as u32;
            let slen3 = (sfc & 0x03) as u32;
            (slen1, slen2, slen3, 0)
        } else {
            let n = sfc - 500;
            let slen1 = (n / 3) as u32;
            let slen2 = (n % 3) as u32;
            (slen1, slen2, 0, 0)
        };
        if gi.window_switching && gi.block_type == 2 {
            let widths = [slen1, slen2, slen3, slen4];
            for sfb in 0..12 {
                let width = widths[sfb / 3];
                if width > 0 {
                    for window in 0..3 {
                        scal.short[gr][ch][sfb][window] = bits
                            .read_bits(width)
                            .ok_or(Mp3Error::InvalidFrame)? as u8;
                    }
                } else {
                    for window in 0..3 {
                        scal.short[gr][ch][sfb][window] = 0;
                    }
                }
            }
        } else {
            const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
            let widths = [slen1, slen2, slen3, slen4];
            for (group, &(first, last)) in GROUPS.iter().enumerate() {
                let width = widths[group];
                if width > 0 {
                    for sfb in first..last {
                        scal.long[gr][ch][sfb] = bits
                            .read_bits(width)
                            .ok_or(Mp3Error::InvalidFrame)? as u8;
                    }
                } else {
                    for sfb in first..last {
                        scal.long[gr][ch][sfb] = 0;
                    }
                }
            }
        }
        return Ok(());
    }

    let sizes = SCALEFAC_SIZES[gi.scalefac_compress as usize];
    let slen1 = sizes[0] as u32;
    let slen2 = sizes[1] as u32;
    if gi.window_switching && gi.block_type == 2 {
        if gi.mixed_block {
            for sfb in 0..8 {
                scal.long[gr][ch][sfb] = bits
                    .read_bits(slen1)
                    .ok_or(Mp3Error::InvalidFrame)? as u8;
            }
            for sfb in 3..12 {
                let width = if sfb < 6 { slen1 } else { slen2 };
                for window in 0..3 {
                    scal.short[gr][ch][sfb][window] = bits
                        .read_bits(width)
                        .ok_or(Mp3Error::InvalidFrame)?
                        as u8;
                }
            }
        } else {
            for sfb in 0..12 {
                let width = if sfb < 6 { slen1 } else { slen2 };
                for window in 0..3 {
                    scal.short[gr][ch][sfb][window] = bits
                        .read_bits(width)
                        .ok_or(Mp3Error::InvalidFrame)?
                        as u8;
                }
            }
        }
    } else {
        const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
        for (group, &(first, last)) in GROUPS.iter().enumerate() {
            let width = if group < 2 { slen1 } else { slen2 };
            if gr == 1 && side.scfsi[ch][group] != 0 {
                for sfb in first..last {
                    scal.long[gr][ch][sfb] = scal.long[0][ch][sfb];
                }
            } else {
                for sfb in first..last {
                    scal.long[gr][ch][sfb] = bits
                        .read_bits(width)
                        .ok_or(Mp3Error::InvalidFrame)?
                        as u8;
                }
            }
        }
    }
    Ok(())
}

fn read_huffman(
    bits: &mut BitReader<'_>,
    side: &mut SideInfo,
    spec: &mut [f32; 576],
    gr: usize,
    ch: usize,
    part_start: usize,
    sample_rate_index: usize,
) -> Result<(), Mp3Error> {
    *spec = [0.0; 576];
    let gi = side.gr[gr][ch];
    let part_bits = gi.part2_3_length as usize;
    let end_bit = part_start + part_bits - 1;
    if end_bit >= bits.total_bits() {
        return Err(Mp3Error::HuffmanError);
    }

    let sfreq = sample_rate_index;
    let switched_short = gi.window_switching && gi.block_type == 2;
    let (region1_start, region2_start) = if switched_short {
        (36usize, 576usize)
    } else {
        let band = &BANDS[sfreq];
        let r0 = gi.region0_count as usize;
        let r1 = gi.region1_count as usize;
        (
            band.long[cmp::min(r0 + 1, 22)] as usize,
            band.long[cmp::min(r0 + r1 + 2, 22)] as usize,
        )
    };

    let big_values = gi.big_values as usize;
    let mut pos = 0usize;
    for _ in 0..big_values {
        let table = if pos < region1_start {
            gi.table_select[0]
        } else if pos < region2_start {
            gi.table_select[1]
        } else {
            gi.table_select[2]
        };
        let (x, y, _, _) = huffman_decode(bits, table as usize)?;
        if pos < 576 {
            spec[pos] = x as f32;
            pos += 1;
        }
        if pos < 576 {
            spec[pos] = y as f32;
            pos += 1;
        }
    }

    let table = 32 + gi.count1table_select as usize;
    while pos <= 572 && bits.position() <= end_bit {
        let saved = bits.position();
        let (x, y, v, w) = huffman_decode(bits, table)?;
        if bits.position() > end_bit + 1 {
            bits.set_position(saved);
            break;
        }
        for value in [v, w, x, y] {
            if pos >= 576 {
                break;
            }
            spec[pos] = value as f32;
            pos += 1;
        }
    }
    side.count1[gr][ch] = pos as u16;
    bits.set_position(end_bit + 1);
    Ok(())
}

fn signed_pow43(value: f32) -> f32 {
    if value < 0.0 {
        -pow43(-value)
    } else {
        pow43(value)
    }
}

fn requantize(
    spec: &mut [f32; 576],
    scal: &Scalefacs,
    side: &SideInfo,
    gr: usize,
    ch: usize,
    gi: &GranuleInfo,
    sample_rate_index: usize,
) {
    let count1 = side.count1[gr][ch] as usize;
    if count1 == 0 {
        return;
    }
    let sf_mult = if gi.scalefac_scale { 1.0f32 } else { 0.5f32 };
    let global_gain = gi.global_gain as f32 - 210.0;
    let short = gi.window_switching && gi.block_type == 2;
    if short {
        let first_short = if gi.mixed_block {
            // The first two subbands of a mixed block are long.
            let long_count = cmp::min(count1, 36);
            let mut sfb = 0usize;
            for i in 0..long_count {
                while sfb + 1 < 23 && i >= BANDS[sample_rate_index].long[sfb + 1] as usize {
                    sfb += 1;
                }
                let pf = if gi.preflag { PRETAB[sfb] as f32 } else { 0.0 };
                let exponent = 0.25 * global_gain - sf_mult * (scal.long[gr][ch][sfb] as f32 + pf);
                spec[i] = signed_pow43(spec[i]) * exp2(exponent);
            }
            3
        } else {
            0
        };
        let first_line = if gi.mixed_block { 36 } else { 0 };
        let mut sfb = first_short;
        for i in first_line..count1 {
            let short_bands = &BANDS[sample_rate_index].short;
            while sfb < 12 && i >= short_bands[sfb + 1] as usize * 3 {
                sfb += 1;
            }
            let band_start = short_bands[sfb] as usize * 3;
            let win_len = (short_bands[sfb + 1] - short_bands[sfb]) as usize;
            let window = if win_len == 0 {
                0
            } else {
                cmp::min(2, (i - band_start) / win_len)
            };
            let exponent = 0.25 * (global_gain - 8.0 * gi.subblock_gain[window] as f32)
                - sf_mult * scal.short[gr][ch][sfb][window] as f32;
            spec[i] = signed_pow43(spec[i]) * exp2(exponent);
        }
    } else {
        let mut sfb = 0usize;
        for i in 0..count1 {
            while sfb + 1 < 23 && i >= BANDS[sample_rate_index].long[sfb + 1] as usize {
                sfb += 1;
            }
            let pf = if gi.preflag { PRETAB[sfb] as f32 } else { 0.0 };
            let exponent = 0.25 * global_gain - sf_mult * (scal.long[gr][ch][sfb] as f32 + pf);
            spec[i] = signed_pow43(spec[i]) * exp2(exponent);
        }
    }
}

fn reorder(spec: &mut [f32; 576], sample_rate_index: usize, gi: &GranuleInfo) {
    if !(gi.window_switching && gi.block_type == 2) {
        return;
    }
    let first = if gi.mixed_block { 3 } else { 0 };
    let bands = &BANDS[sample_rate_index].short;
    let mut reordered = [0.0f32; 576];
    for sfb in first..13 {
        let start = bands[sfb] as usize * 3;
        let win_len = (bands[sfb + 1] - bands[sfb]) as usize;
        for j in 0..win_len {
            for window in 0..3 {
                reordered[j * 3 + window] = spec[start + window * win_len + j];
            }
        }
        for j in 0..3 * win_len {
            spec[start + j] = reordered[j];
        }
    }
}

// ---------------------------------------------------------------------------
// Stereo processing
// ---------------------------------------------------------------------------

const INTENSITY_RATIOS: [f32; 6] =
    [0.0, 0.26794919, 0.57735026, 1.0, 1.7320508, 3.7320508];

fn intensity_pan(is_pos: u8) -> Option<(f32, f32)> {
    match is_pos {
        0..=5 => {
            let ratio = INTENSITY_RATIOS[is_pos as usize];
            Some((ratio / (1.0 + ratio), 1.0 / (1.0 + ratio)))
        }
        6 => Some((1.0, 0.0)),
        _ => None,
    }
}

fn stereo_process(
    spec: &mut [[f32; 576]; 2],
    scal: &Scalefacs,
    side: &SideInfo,
    gr: usize,
    mode_extension: u8,
    sample_rate_index: usize,
) {
    if mode_extension & 0x02 != 0 {
        let max_pos = cmp::max(side.count1[gr][0], side.count1[gr][1]) as usize;
        let inv_sqrt2 = 0.70710677f32;
        for i in 0..cmp::min(max_pos, 576) {
            let mid = spec[0][i];
            let side_value = spec[1][i];
            spec[0][i] = (mid + side_value) * inv_sqrt2;
            spec[1][i] = (mid - side_value) * inv_sqrt2;
        }
    }
    if mode_extension & 0x01 == 0 {
        return;
    }
    let gi = side.gr[gr][0];
    let right_count1 = side.count1[gr][1] as usize;
    let bands = &BANDS[sample_rate_index];
    if gi.window_switching && gi.block_type == 2 {
        if gi.mixed_block {
            for sfb in 0..8 {
                if bands.long[sfb] as usize >= right_count1 {
                    apply_intensity_long(spec, scal, gr, sfb, sample_rate_index);
                }
            }
            for sfb in 3..12 {
                if bands.short[sfb] as usize * 3 >= right_count1 {
                    apply_intensity_short(spec, scal, gr, sfb, sample_rate_index);
                }
            }
        } else {
            for sfb in 0..12 {
                if bands.short[sfb] as usize * 3 >= right_count1 {
                    apply_intensity_short(spec, scal, gr, sfb, sample_rate_index);
                }
            }
        }
    } else {
        for sfb in 0..21 {
            if bands.long[sfb] as usize >= right_count1 {
                apply_intensity_long(spec, scal, gr, sfb, sample_rate_index);
            }
        }
    }
}

fn apply_intensity_long(
    spec: &mut [[f32; 576]; 2],
    scal: &Scalefacs,
    gr: usize,
    sfb: usize,
    sample_rate_index: usize,
) {
    // The right channel carries the intensity positions in its scalefactors,
    // while its spectral lines are not transmitted for these bands.
    let is_pos = scal.long[gr][1][sfb];
    let (pan_left, pan_right) = match intensity_pan(is_pos) {
        Some(pan) => pan,
        None => return,
    };
    let start = BANDS[sample_rate_index].long[sfb] as usize;
    let stop = BANDS[sample_rate_index].long[sfb + 1] as usize;
    for i in start..stop {
        let value = spec[0][i];
        spec[0][i] = pan_left * value;
        spec[1][i] = pan_right * value;
    }
}

fn apply_intensity_short(
    spec: &mut [[f32; 576]; 2],
    scal: &Scalefacs,
    gr: usize,
    sfb: usize,
    sample_rate_index: usize,
) {
    let band = &BANDS[sample_rate_index].short;
    let win_len = (band[sfb + 1] - band[sfb]) as usize;
    for window in 0..3 {
        let is_pos = scal.short[gr][1][sfb][window];
        let (pan_left, pan_right) = match intensity_pan(is_pos) {
            Some(pan) => pan,
            None => continue,
        };
        let start = band[sfb] as usize * 3 + win_len * window;
        for i in start..start + win_len {
            let value = spec[0][i];
            spec[0][i] = pan_left * value;
            spec[1][i] = pan_right * value;
        }
    }
}

// ---------------------------------------------------------------------------
// Antialias, IMDCT and polyphase synthesis
// ---------------------------------------------------------------------------

fn antialias(spec: &mut [f32; 576], gi: &GranuleInfo) {
    let switched_short = gi.window_switching && gi.block_type == 2;
    if switched_short && !gi.mixed_block {
        return;
    }
    let limit = if switched_short && gi.mixed_block { 2 } else { 32 };
    for sb in 1..limit {
        for i in 0..8 {
            let lower = 18 * sb - 1 - i;
            let upper = 18 * sb + i;
            let lo = spec[lower];
            let hi = spec[upper];
            spec[lower] = lo * ALIAS_CS[i] - hi * ALIAS_CA[i];
            spec[upper] = hi * ALIAS_CS[i] + lo * ALIAS_CA[i];
        }
    }
}

fn imdct(input: &[f32], out: &mut [f32; 36], block_type: u8) {
    if block_type == 2 {
        for value in out.iter_mut() {
            *value = 0.0;
        }
        for block in 0..3 {
            for p in 0..12 {
                let mut sum = 0.0f32;
                for m in 0..6 {
                    sum += input[block + 3 * m] * IMDCT12_COS[m][p];
                }
                out[6 * block + p + 6] += sum * IMDCT_WINDOW[2][p];
            }
        }
    } else {
        let window = &IMDCT_WINDOW[block_type as usize];
        for p in 0..36 {
            let mut sum = 0.0f32;
            for m in 0..18 {
                sum += input[m] * IMDCT36_COS[m][p];
            }
            out[p] = sum * window[p];
        }
    }
}

fn hybrid_synthesis(
    spec: &mut [f32; 576],
    overlap: &mut [[f32; 18]; MAX_CHANNELS * 32],
    ch: usize,
    gi: &GranuleInfo,
) {
    let switched_short = gi.window_switching && gi.block_type == 2;
    for sb in 0..32 {
        let block_type = if switched_short && gi.mixed_block && sb < 2 {
            0
        } else {
            gi.block_type
        };
        let mut raw = [0.0f32; 36];
        imdct(&spec[sb * 18..sb * 18 + 18], &mut raw, block_type);
        let store = &mut overlap[ch * 32 + sb];
        for i in 0..18 {
            spec[sb * 18 + i] = raw[i] + store[i];
            store[i] = raw[i + 18];
        }
    }
}

fn frequency_inversion(spec: &mut [f32; 576]) {
    let mut sb = 1;
    while sb < 32 {
        let mut i = 1;
        while i < 18 {
            spec[sb * 18 + i] = -spec[sb * 18 + i];
            i += 2;
        }
        sb += 2;
    }
}

fn subband_synthesis(
    spec: &[f32; 576],
    v: &mut [f32; 1024],
    out: &mut [i16],
    base_sample: usize,
    stride: usize,
    channel: usize,
) {
    let mut u = [0.0f32; 512];
    for slot in 0..18 {
        for i in (64..1024).rev() {
            v[i] = v[i - 64];
        }
        for i in 0..64 {
            let mut sum = 0.0f32;
            for j in 0..32 {
                sum += SYNTH_COS[i][j] * spec[j * 18 + slot];
            }
            v[i] = sum;
        }
        for i in 0..8 {
            for j in 0..32 {
                u[i * 64 + j] = v[i * 128 + j];
                u[i * 64 + j + 32] = v[i * 128 + j + 96];
            }
        }
        for i in 0..512 {
            u[i] *= SYNTH_WINDOW[i];
        }
        for i in 0..32 {
            let mut sum = 0.0f32;
            for j in 0..16 {
                sum += u[j * 32 + i];
            }
            let index = (base_sample + slot * 32 + i) * stride + channel;
            out[index] = to_i16(sum * 32767.0);
        }
    }
}

fn to_i16(value: f32) -> i16 {
    if value >= 32767.0 {
        32767
    } else if value <= -32768.0 {
        -32768
    } else if value >= 0.0 {
        (value + 0.5) as i16
    } else {
        (value - 0.5) as i16
    }
}

// ---------------------------------------------------------------------------
// Small numeric kernels (no libm in `no_std`)
// ---------------------------------------------------------------------------

/// `2^x` for the requantisation exponent, accurate to about 1e-7.
fn exp2(x: f32) -> f32 {
    let x = if x < -126.0 {
        -126.0
    } else if x > 127.0 {
        127.0
    } else {
        x
    };
    let mut n = x as i32;
    if x >= 0.0 {
        if x - n as f32 >= 0.5 {
            n += 1;
        }
    } else if n as f32 - x >= 0.5 {
        n -= 1;
    }
    let f = x - n as f32;
    // Taylor series of 2^f around 0; |f| <= 0.5 keeps the error tiny.
    const COEFF: [f32; 7] = [
        1.0,
        0.69314718,
        0.24022651,
        0.055504109,
        0.009618129,
        0.0013333558,
        0.0001540353,
    ];
    let mut p = COEFF[6];
    let mut i = 5;
    loop {
        p = p * f + COEFF[i];
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let scale = f32::from_bits(((n + 127) as u32) << 23);
    scale * p
}

/// `x^(4/3)` for the non-negative integer Huffman values (0..=8206).
fn pow43(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    // Cube root by Newton iteration from a bit-hack estimate, then x * cbrt(x).
    let mut y = f32::from_bits(x.to_bits() / 3 + 0x2A51_4067);
    let mut iteration = 0;
    while iteration < 4 {
        y = (2.0 * y + x / (y * y)) / 3.0;
        iteration += 1;
    }
    x * y
}

// ---------------------------------------------------------------------------
// Unit tests for the individual stages (`cargo test --lib mp3`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_accepts_mpeg1_layer3() {
        let header = parse_header(&[0xFF, 0xFB, 0x90, 0x64]).unwrap();
        assert_eq!(header.bitrate_kbps, 128);
        assert_eq!(header.sample_rate, 44100);
        assert_eq!(header.channels, 2);
        assert_eq!(header.frame_len, 417);
        assert!(!header.crc);
    }

    #[test]
    fn header_reports_refused_shapes_separately() {
        assert_eq!(
            parse_header(&[0xFF, 0xFB, 0x00, 0x64]).err(),
            Some(Mp3Error::FreeFormat)
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0x90, 0x64]).err(),
            Some(Mp3Error::UnsupportedLayer)
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFF, 0x90, 0x64]).err(),
            Some(Mp3Error::UnsupportedLayer)
        );
        assert_eq!(
            parse_header(&[0xFF, 0xE3, 0x90, 0x64]).err(),
            Some(Mp3Error::UnsupportedVersion)
        );
        assert!(parse_header(&[0xFF, 0xF3, 0x90, 0x64]).is_ok());
        assert_eq!(
            parse_header(&[0xFF, 0xEB, 0x90, 0x64]).err(),
            Some(Mp3Error::InvalidFrame)
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFB, 0xF0, 0x64]).err(),
            Some(Mp3Error::InvalidFrame)
        );
        assert_eq!(
            parse_header(&[0x00, 0x11, 0x22, 0x33]).err(),
            Some(Mp3Error::SyncLost)
        );
    }

    #[test]
    fn id3_size_is_syncsafe_and_bounded() {
        let mut header = *b"ID3\x04\x00\x00\x00\x00\x00\x22";
        assert_eq!(id3v2_len(&header), Ok(44));
        header[9] = 0x7F;
        header[8] = 0x7F;
        header[7] = 0x7F;
        header[6] = 0x7F;
        assert_eq!(id3v2_len(&header), Err(Mp3Error::Id3TooLarge));
        header[6] = 0;
        header[7] = 0;
        header[8] = 0;
        header[9] = 0x22;
        header[5] = 0x10;
        assert_eq!(id3v2_len(&header), Ok(54));
    }

    #[test]
    fn bit_reader_reads_most_significant_bit_first() {
        let data = [0b1011_0010u8, 0b0100_0000];
        let mut bits = BitReader::new(&data);
        assert_eq!(bits.read_bits(3), Some(0b101));
        assert_eq!(bits.read_bits(5), Some(0b1_0010));
        assert_eq!(bits.read_bits(4), Some(0b0100));
        assert_eq!(bits.read_bits(4), Some(0));
        assert_eq!(bits.read_bit(), None);
    }

    #[test]
    fn huffman_table_one_follows_the_iso_tree() {
        // Codes 1, 01, 001, 000 with their sign bits: (0,0), (+1,0),
        // (0,-1), (-1,+1).
        let data = [0b1010_0011u8, 0b0001_0000];
        let mut bits = BitReader::new(&data);
        assert_eq!(huffman_decode(&mut bits, 1), Ok((0, 0, 0, 0)));
        assert_eq!(huffman_decode(&mut bits, 1), Ok((1, 0, 0, 0)));
        assert_eq!(huffman_decode(&mut bits, 1), Ok((0, -1, 0, 0)));
        assert_eq!(huffman_decode(&mut bits, 1), Ok((-1, 1, 0, 0)));
    }

    #[test]
    fn exp2_and_pow43_match_reference_values() {
        let close = |a: f32, b: f32| {
            let diff = if a > b { a - b } else { b - a };
            let magnitude = if b < 0.0 { -b } else { b };
            diff <= magnitude * 1e-5 + 1e-6
        };
        assert!(close(exp2(0.0), 1.0));
        assert!(close(exp2(1.0), 2.0));
        assert!(close(exp2(-1.0), 0.5));
        assert!(close(exp2(0.25), 1.1892071));
        assert!(close(exp2(-10.25), 0.0008211879));
        assert!(close(pow43(0.0), 0.0));
        assert!(close(pow43(1.0), 1.0));
        assert!(close(pow43(2.0), 2.5198421));
        assert!(close(pow43(8.0), 16.0));
        assert!(close(pow43(8206.0), 165516.78));
        assert!(close(signed_pow43(-8.0), -16.0));
    }

    #[test]
    fn reservoir_history_is_bounded() {
        let mut decoder = Mp3Decoder::new();
        assert_eq!(decoder.push(&[0u8; 2048]).is_ok(), true);
        assert_eq!(decoder.push(&[0u8; 2048]).is_ok(), true);
        assert_eq!(decoder.push(&[0u8; 1]), Err(Mp3Error::BufferFull));
    }

    #[test]
    fn zz_dump_mono_frame() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mp3/tone_44100_mono_64k.mp3"
        );
        let data = std::fs::read(path).unwrap();
        let mut hdr = [0u8; 10];
        hdr.copy_from_slice(&data[..10]);
        let id3 = id3v2_len(&hdr).unwrap();
        let audio = &data[id3..];
        let mut off = 0usize;
        for frame_no in 0..6 {
            let header = parse_header(&audio[off..off + 4]).unwrap();
            let frame = &audio[off..off + header.frame_len];
            let mut sr = BitReader::new(&frame[4..21]);
            let side = SideInfo::parse(&mut sr, 1, 3).unwrap();
            std::println!(
                "frame {} len={} mdb={}",
                frame_no,
                frame.len(),
                side.main_data_begin
            );
            for gr in 0..2 {
                let gi = &side.gr[gr][0];
                std::println!(
                    "  gr{} p23={} bv={} gg={} sfc={} ws={} bt={} mix={} ts={:?} sbg={:?} r0={} r1={} pf={} ss={} c1ts={}",
                    gr, gi.part2_3_length, gi.big_values, gi.global_gain,
                    gi.scalefac_compress, gi.window_switching, gi.block_type,
                    gi.mixed_block, gi.table_select, gi.subblock_gain,
                    gi.region0_count, gi.region1_count, gi.preflag,
                    gi.scalefac_scale, gi.count1table_select
                );
            }
            if side.gr[0][0].part2_3_length != 0 {
                let main = &frame[21..];
                let mut sr2 = BitReader::new(&frame[4..21]);
                let mut side2 = SideInfo::parse(&mut sr2, 1, 3).unwrap();
                let mut scal = Scalefacs::zero();
                let mut spectra = [[0.0f32; 576]; 2];
                let mut bits = BitReader::new(main);
                for gr in 0..2 {
                    let gi = side2.gr[gr][0];
                    if gi.part2_3_length == 0 {
                        continue;
                    }
                    let part_start = bits.position();
                    read_scalefactors(&mut bits, &mut scal, &side2, gr, 0, &gi, 3).unwrap();
                    read_huffman(&mut bits, &mut side2, &mut spectra[gr], gr, 0, part_start, 0)
                        .unwrap();
                    std::println!(
                        "  decoded gr{}: bit={} count1={}",
                        gr,
                        bits.position() - part_start,
                        side2.count1[gr][0]
                    );
                }
                for gr in 0..2 {
                    let gi = side2.gr[gr][0];
                    let mut sp = spectra[gr];
                    requantize(&mut sp, &scal, &side2, gr, 0, &gi, 0);
                    print_stage("requant", gr, &sp);
                    reorder(&mut sp, 0, &gi);
                    print_stage("reorder", gr, &sp);
                    antialias(&mut sp, &gi);
                    let mut raw = [0.0f32; 36];
                    imdct(&sp[0..18], &mut raw, gi.block_type);
                    let mut raw_max = 0.0f32;
                    for v in raw.iter() {
                        if v.abs() > raw_max {
                            raw_max = v.abs();
                        }
                    }
                    std::println!("    raw subband0 max={:.5} sp0={:?}", raw_max, &sp[..8]);
                    let mut overlap = [[0.0f32; 18]; MAX_CHANNELS * 32];
                    let mut v = [0.0f32; 1024];
                    hybrid_synthesis(&mut sp, &mut overlap, 0, &gi);
                    print_stage("hybrid", gr, &sp);
                    frequency_inversion(&mut sp);
                    let mut out = [0i16; MAX_FRAME_SAMPLES];
                    subband_synthesis(&sp, &mut v, &mut out, 0, 1, 0);
                    let mut sum = 0.0f64;
                    for s in out.iter() {
                        sum += (*s as f64 / 32768.0) * (*s as f64 / 32768.0);
                    }
                    std::println!(
                        "    pcm gr{} rms={:.4} first={:?}",
                        gr,
                        (sum / out.len() as f64).sqrt(),
                        &out[..16]
                    );
                }
                return;
            }
            off += header.frame_len;
        }
    }

    fn print_stage(name: &str, gr: usize, values: &[f32; 576]) {
        let mut max = 0.0f32;
        let mut sum = 0.0f64;
        let mut first = [0.0f32; 8];
        for (i, v) in values.iter().enumerate() {
            if v.abs() > max {
                max = v.abs();
            }
            sum += (*v as f64) * (*v as f64);
            if i < 8 {
                first[i] = *v;
            }
        }
        std::println!(
            "    {} gr{} max={:.4} rms={:.4} first={:?}",
            name,
            gr,
            max,
            (sum / 576.0).sqrt(),
            first
        );
    }
}
// ISO/IEC 11172-3 Layer III constant tables (generated, public-domain tables).

pub(crate) static HUFF_TREE: [u16; 2804] = [
    0x0201, 0x0000, 0x0201, 0x0010, 0x0201, 0x0001, 0x0011, 0x0201, 0x0000, 0x0401,
    0x0201, 0x0010, 0x0001, 0x0201, 0x0011, 0x0401, 0x0201, 0x0020, 0x0021, 0x0201,
    0x0012, 0x0201, 0x0002, 0x0022, 0x0401, 0x0201, 0x0000, 0x0001, 0x0201, 0x0011,
    0x0201, 0x0010, 0x0401, 0x0201, 0x0020, 0x0021, 0x0201, 0x0012, 0x0201, 0x0002,
    0x0022, 0x0201, 0x0000, 0x0401, 0x0201, 0x0010, 0x0001, 0x0201, 0x0011, 0x0801,
    0x0401, 0x0201, 0x0020, 0x0002, 0x0201, 0x0021, 0x0012, 0x0801, 0x0401, 0x0201,
    0x0022, 0x0030, 0x0201, 0x0003, 0x0013, 0x0201, 0x0031, 0x0201, 0x0032, 0x0201,
    0x0023, 0x0033, 0x0601, 0x0401, 0x0201, 0x0000, 0x0010, 0x0011, 0x0601, 0x0201,
    0x0001, 0x0201, 0x0020, 0x0021, 0x0601, 0x0201, 0x0012, 0x0201, 0x0002, 0x0022,
    0x0401, 0x0201, 0x0031, 0x0013, 0x0401, 0x0201, 0x0030, 0x0032, 0x0201, 0x0023,
    0x0201, 0x0003, 0x0033, 0x0201, 0x0000, 0x0401, 0x0201, 0x0010, 0x0001, 0x0801,
    0x0201, 0x0011, 0x0401, 0x0201, 0x0020, 0x0002, 0x0021, 0x1201, 0x0601, 0x0201,
    0x0012, 0x0201, 0x0022, 0x0030, 0x0401, 0x0201, 0x0031, 0x0013, 0x0401, 0x0201,
    0x0003, 0x0032, 0x0201, 0x0023, 0x0004, 0x0a01, 0x0401, 0x0201, 0x0040, 0x0041,
    0x0201, 0x0014, 0x0201, 0x0042, 0x0024, 0x0c01, 0x0601, 0x0401, 0x0201, 0x0033,
    0x0043, 0x0050, 0x0401, 0x0201, 0x0034, 0x0005, 0x0051, 0x0601, 0x0201, 0x0015,
    0x0201, 0x0052, 0x0025, 0x0401, 0x0201, 0x0044, 0x0035, 0x0401, 0x0201, 0x0053,
    0x0054, 0x0201, 0x0045, 0x0055, 0x0601, 0x0201, 0x0000, 0x0201, 0x0010, 0x0001,
    0x0201, 0x0011, 0x0401, 0x0201, 0x0021, 0x0012, 0x0e01, 0x0401, 0x0201, 0x0020,
    0x0002, 0x0201, 0x0022, 0x0401, 0x0201, 0x0030, 0x0003, 0x0201, 0x0031, 0x0013,
    0x0e01, 0x0801, 0x0401, 0x0201, 0x0032, 0x0023, 0x0201, 0x0040, 0x0004, 0x0201,
    0x0041, 0x0201, 0x0014, 0x0042, 0x0c01, 0x0601, 0x0201, 0x0024, 0x0201, 0x0033,
    0x0050, 0x0401, 0x0201, 0x0043, 0x0034, 0x0051, 0x0601, 0x0201, 0x0015, 0x0201,
    0x0005, 0x0052, 0x0601, 0x0201, 0x0025, 0x0201, 0x0044, 0x0035, 0x0201, 0x0053,
    0x0201, 0x0045, 0x0201, 0x0054, 0x0055, 0x0801, 0x0401, 0x0201, 0x0000, 0x0010,
    0x0201, 0x0001, 0x0011, 0x0a01, 0x0401, 0x0201, 0x0020, 0x0021, 0x0201, 0x0012,
    0x0201, 0x0002, 0x0022, 0x0c01, 0x0601, 0x0401, 0x0201, 0x0030, 0x0003, 0x0031,
    0x0201, 0x0013, 0x0201, 0x0032, 0x0023, 0x0c01, 0x0401, 0x0201, 0x0041, 0x0014,
    0x0401, 0x0201, 0x0040, 0x0033, 0x0201, 0x0042, 0x0024, 0x0a01, 0x0601, 0x0401,
    0x0201, 0x0004, 0x0050, 0x0043, 0x0201, 0x0034, 0x0051, 0x0801, 0x0401, 0x0201,
    0x0015, 0x0052, 0x0201, 0x0025, 0x0044, 0x0601, 0x0401, 0x0201, 0x0005, 0x0054,
    0x0053, 0x0201, 0x0035, 0x0201, 0x0045, 0x0055, 0x0201, 0x0000, 0x0401, 0x0201,
    0x0010, 0x0001, 0x0a01, 0x0201, 0x0011, 0x0401, 0x0201, 0x0020, 0x0002, 0x0201,
    0x0021, 0x0012, 0x1c01, 0x0801, 0x0401, 0x0201, 0x0022, 0x0030, 0x0201, 0x0031,
    0x0013, 0x0801, 0x0401, 0x0201, 0x0003, 0x0032, 0x0201, 0x0023, 0x0040, 0x0401,
    0x0201, 0x0041, 0x0014, 0x0401, 0x0201, 0x0004, 0x0033, 0x0201, 0x0042, 0x0024,
    0x1c01, 0x0a01, 0x0601, 0x0401, 0x0201, 0x0050, 0x0005, 0x0060, 0x0201, 0x0061,
    0x0016, 0x0c01, 0x0601, 0x0401, 0x0201, 0x0043, 0x0034, 0x0051, 0x0201, 0x0015,
    0x0201, 0x0052, 0x0025, 0x0401, 0x0201, 0x0026, 0x0036, 0x0071, 0x1401, 0x0801,
    0x0201, 0x0017, 0x0401, 0x0201, 0x0044, 0x0053, 0x0006, 0x0601, 0x0401, 0x0201,
    0x0035, 0x0045, 0x0062, 0x0201, 0x0070, 0x0201, 0x0007, 0x0064, 0x0e01, 0x0401,
    0x0201, 0x0072, 0x0027, 0x0601, 0x0201, 0x0063, 0x0201, 0x0054, 0x0055, 0x0201,
    0x0046, 0x0073, 0x0801, 0x0401, 0x0201, 0x0037, 0x0065, 0x0201, 0x0056, 0x0074,
    0x0601, 0x0201, 0x0047, 0x0201, 0x0066, 0x0075, 0x0401, 0x0201, 0x0057, 0x0076,
    0x0201, 0x0067, 0x0077, 0x0601, 0x0201, 0x0000, 0x0201, 0x0010, 0x0001, 0x0801,
    0x0201, 0x0011, 0x0401, 0x0201, 0x0020, 0x0002, 0x0012, 0x1801, 0x0801, 0x0201,
    0x0021, 0x0201, 0x0022, 0x0201, 0x0030, 0x0003, 0x0401, 0x0201, 0x0031, 0x0013,
    0x0401, 0x0201, 0x0032, 0x0023, 0x0401, 0x0201, 0x0040, 0x0004, 0x0201, 0x0041,
    0x0014, 0x1e01, 0x1001, 0x0a01, 0x0401, 0x0201, 0x0042, 0x0024, 0x0401, 0x0201,
    0x0033, 0x0043, 0x0050, 0x0401, 0x0201, 0x0034, 0x0051, 0x0061, 0x0601, 0x0201,
    0x0016, 0x0201, 0x0006, 0x0026, 0x0201, 0x0062, 0x0201, 0x0015, 0x0201, 0x0005,
    0x0052, 0x1001, 0x0a01, 0x0601, 0x0401, 0x0201, 0x0025, 0x0044, 0x0060, 0x0201,
    0x0063, 0x0036, 0x0401, 0x0201, 0x0070, 0x0017, 0x0071, 0x1001, 0x0601, 0x0401,
    0x0201, 0x0007, 0x0064, 0x0072, 0x0201, 0x0027, 0x0401, 0x0201, 0x0053, 0x0035,
    0x0201, 0x0054, 0x0045, 0x0a01, 0x0401, 0x0201, 0x0046, 0x0073, 0x0201, 0x0037,
    0x0201, 0x0065, 0x0056, 0x0a01, 0x0601, 0x0401, 0x0201, 0x0055, 0x0057, 0x0074,
    0x0201, 0x0047, 0x0066, 0x0401, 0x0201, 0x0075, 0x0076, 0x0201, 0x0067, 0x0077,
    0x0c01, 0x0401, 0x0201, 0x0010, 0x0001, 0x0201, 0x0011, 0x0201, 0x0000, 0x0201,
    0x0020, 0x0002, 0x1001, 0x0401, 0x0201, 0x0021, 0x0012, 0x0401, 0x0201, 0x0022,
    0x0031, 0x0201, 0x0013, 0x0201, 0x0030, 0x0201, 0x0003, 0x0040, 0x1a01, 0x0801,
    0x0401, 0x0201, 0x0032, 0x0023, 0x0201, 0x0041, 0x0033, 0x0a01, 0x0401, 0x0201,
    0x0014, 0x0042, 0x0201, 0x0024, 0x0201, 0x0004, 0x0050, 0x0401, 0x0201, 0x0043,
    0x0034, 0x0201, 0x0051, 0x0015, 0x1c01, 0x0e01, 0x0801, 0x0401, 0x0201, 0x0052,
    0x0025, 0x0201, 0x0053, 0x0035, 0x0401, 0x0201, 0x0060, 0x0016, 0x0061, 0x0401,
    0x0201, 0x0062, 0x0026, 0x0601, 0x0401, 0x0201, 0x0005, 0x0006, 0x0044, 0x0201,
    0x0054, 0x0045, 0x1201, 0x0a01, 0x0401, 0x0201, 0x0063, 0x0036, 0x0401, 0x0201,
    0x0070, 0x0007, 0x0071, 0x0401, 0x0201, 0x0017, 0x0064, 0x0201, 0x0046, 0x0072,
    0x0a01, 0x0601, 0x0201, 0x0027, 0x0201, 0x0055, 0x0073, 0x0201, 0x0037, 0x0056,
    0x0801, 0x0401, 0x0201, 0x0065, 0x0074, 0x0201, 0x0047, 0x0066, 0x0401, 0x0201,
    0x0075, 0x0057, 0x0201, 0x0076, 0x0201, 0x0067, 0x0077, 0x0201, 0x0000, 0x0601,
    0x0201, 0x0010, 0x0201, 0x0001, 0x0011, 0x1c01, 0x0801, 0x0401, 0x0201, 0x0020,
    0x0002, 0x0201, 0x0021, 0x0012, 0x0801, 0x0401, 0x0201, 0x0022, 0x0030, 0x0201,
    0x0003, 0x0031, 0x0601, 0x0201, 0x0013, 0x0201, 0x0032, 0x0023, 0x0401, 0x0201,
    0x0040, 0x0004, 0x0041, 0x4601, 0x1c01, 0x0e01, 0x0601, 0x0201, 0x0014, 0x0201,
    0x0033, 0x0042, 0x0401, 0x0201, 0x0024, 0x0050, 0x0201, 0x0043, 0x0034, 0x0401,
    0x0201, 0x0051, 0x0015, 0x0401, 0x0201, 0x0005, 0x0052, 0x0201, 0x0025, 0x0201,
    0x0044, 0x0053, 0x0e01, 0x0801, 0x0401, 0x0201, 0x0060, 0x0006, 0x0201, 0x0061,
    0x0016, 0x0401, 0x0201, 0x0080, 0x0008, 0x0081, 0x1001, 0x0801, 0x0401, 0x0201,
    0x0035, 0x0062, 0x0201, 0x0026, 0x0054, 0x0401, 0x0201, 0x0045, 0x0063, 0x0201,
    0x0036, 0x0070, 0x0601, 0x0401, 0x0201, 0x0007, 0x0055, 0x0071, 0x0201, 0x0017,
    0x0201, 0x0027, 0x0037, 0x4801, 0x1801, 0x0c01, 0x0401, 0x0201, 0x0018, 0x0082,
    0x0201, 0x0028, 0x0401, 0x0201, 0x0064, 0x0046, 0x0072, 0x0801, 0x0401, 0x0201,
    0x0084, 0x0048, 0x0201, 0x0090, 0x0009, 0x0201, 0x0091, 0x0019, 0x1801, 0x0e01,
    0x0801, 0x0401, 0x0201, 0x0073, 0x0065, 0x0201, 0x0056, 0x0074, 0x0401, 0x0201,
    0x0047, 0x0066, 0x0083, 0x0601, 0x0201, 0x0038, 0x0201, 0x0075, 0x0057, 0x0201,
    0x0092, 0x0029, 0x0e01, 0x0801, 0x0401, 0x0201, 0x0067, 0x0085, 0x0201, 0x0058,
    0x0039, 0x0201, 0x0093, 0x0201, 0x0049, 0x0086, 0x0601, 0x0201, 0x00a0, 0x0201,
    0x0068, 0x000a, 0x0201, 0x00a1, 0x001a, 0x4401, 0x1801, 0x0c01, 0x0401, 0x0201,
    0x00a2, 0x002a, 0x0401, 0x0201, 0x0095, 0x0059, 0x0201, 0x00a3, 0x003a, 0x0801,
    0x0401, 0x0201, 0x004a, 0x0096, 0x0201, 0x00b0, 0x000b, 0x0201, 0x00b1, 0x001b,
    0x1401, 0x0801, 0x0201, 0x00b2, 0x0401, 0x0201, 0x0076, 0x0077, 0x0094, 0x0601,
    0x0401, 0x0201, 0x0087, 0x0078, 0x00a4, 0x0401, 0x0201, 0x0069, 0x00a5, 0x002b,
    0x0c01, 0x0601, 0x0401, 0x0201, 0x005a, 0x0088, 0x00b3, 0x0201, 0x003b, 0x0201,
    0x0079, 0x00a6, 0x0601, 0x0401, 0x0201, 0x006a, 0x00b4, 0x00c0, 0x0401, 0x0201,
    0x000c, 0x0098, 0x00c1, 0x3c01, 0x1601, 0x0a01, 0x0601, 0x0201, 0x001c, 0x0201,
    0x0089, 0x00b5, 0x0201, 0x005b, 0x00c2, 0x0401, 0x0201, 0x002c, 0x003c, 0x0401,
    0x0201, 0x00b6, 0x006b, 0x0201, 0x00c4, 0x004c, 0x1001, 0x0801, 0x0401, 0x0201,
    0x00a8, 0x008a, 0x0201, 0x00d0, 0x000d, 0x0201, 0x00d1, 0x0201, 0x004b, 0x0201,
    0x0097, 0x00a7, 0x0c01, 0x0601, 0x0201, 0x00c3, 0x0201, 0x007a, 0x0099, 0x0401,
    0x0201, 0x00c5, 0x005c, 0x00b7, 0x0401, 0x0201, 0x001d, 0x00d2, 0x0201, 0x002d,
    0x0201, 0x007b, 0x00d3, 0x3401, 0x1c01, 0x0c01, 0x0401, 0x0201, 0x003d, 0x00c6,
    0x0401, 0x0201, 0x006c, 0x00a9, 0x0201, 0x009a, 0x00d4, 0x0801, 0x0401, 0x0201,
    0x00b8, 0x008b, 0x0201, 0x004d, 0x00c7, 0x0401, 0x0201, 0x007c, 0x00d5, 0x0201,
    0x005d, 0x00e0, 0x0a01, 0x0401, 0x0201, 0x00e1, 0x001e, 0x0401, 0x0201, 0x000e,
    0x002e, 0x00e2, 0x0801, 0x0401, 0x0201, 0x00e3, 0x006d, 0x0201, 0x008c, 0x00e4,
    0x0401, 0x0201, 0x00e5, 0x00ba, 0x00f0, 0x2601, 0x1001, 0x0401, 0x0201, 0x00f1,
    0x001f, 0x0601, 0x0401, 0x0201, 0x00aa, 0x009b, 0x00b9, 0x0201, 0x003e, 0x0201,
    0x00d6, 0x00c8, 0x0c01, 0x0601, 0x0201, 0x004e, 0x0201, 0x00d7, 0x007d, 0x0201,
    0x00ab, 0x0201, 0x005e, 0x00c9, 0x0601, 0x0201, 0x000f, 0x0201, 0x009c, 0x006e,
    0x0201, 0x00f2, 0x002f, 0x2001, 0x1001, 0x0601, 0x0401, 0x0201, 0x00d8, 0x008d,
    0x003f, 0x0601, 0x0201, 0x00f3, 0x0201, 0x00e6, 0x00ca, 0x0201, 0x00f4, 0x004f,
    0x0801, 0x0401, 0x0201, 0x00bb, 0x00ac, 0x0201, 0x00e7, 0x00f5, 0x0401, 0x0201,
    0x00d9, 0x009d, 0x0201, 0x005f, 0x00e8, 0x1e01, 0x0c01, 0x0601, 0x0201, 0x006f,
    0x0201, 0x00f6, 0x00cb, 0x0401, 0x0201, 0x00bc, 0x00ad, 0x00da, 0x0801, 0x0201,
    0x00f7, 0x0401, 0x0201, 0x007e, 0x007f, 0x008e, 0x0601, 0x0401, 0x0201, 0x009e,
    0x00ae, 0x00cc, 0x0201, 0x00f8, 0x008f, 0x1201, 0x0801, 0x0401, 0x0201, 0x00db,
    0x00bd, 0x0201, 0x00ea, 0x00f9, 0x0401, 0x0201, 0x009f, 0x00eb, 0x0201, 0x00be,
    0x0201, 0x00cd, 0x00fa, 0x0e01, 0x0401, 0x0201, 0x00dd, 0x00ec, 0x0601, 0x0401,
    0x0201, 0x00e9, 0x00af, 0x00dc, 0x0201, 0x00ce, 0x00fb, 0x0801, 0x0401, 0x0201,
    0x00bf, 0x00de, 0x0201, 0x00cf, 0x00ee, 0x0401, 0x0201, 0x00df, 0x00ef, 0x0201,
    0x00ff, 0x0201, 0x00ed, 0x0201, 0x00fd, 0x0201, 0x00fc, 0x00fe, 0x1001, 0x0601,
    0x0201, 0x0000, 0x0201, 0x0010, 0x0001, 0x0201, 0x0011, 0x0401, 0x0201, 0x0020,
    0x0002, 0x0201, 0x0021, 0x0012, 0x3201, 0x1001, 0x0601, 0x0201, 0x0022, 0x0201,
    0x0030, 0x0031, 0x0601, 0x0201, 0x0013, 0x0201, 0x0003, 0x0040, 0x0201, 0x0032,
    0x0023, 0x0e01, 0x0601, 0x0401, 0x0201, 0x0004, 0x0014, 0x0041, 0x0401, 0x0201,
    0x0033, 0x0042, 0x0201, 0x0024, 0x0043, 0x0a01, 0x0601, 0x0201, 0x0034, 0x0201,
    0x0050, 0x0005, 0x0201, 0x0051, 0x0015, 0x0401, 0x0201, 0x0052, 0x0025, 0x0401,
    0x0201, 0x0044, 0x0053, 0x0061, 0x5a01, 0x2401, 0x1201, 0x0a01, 0x0601, 0x0201,
    0x0035, 0x0201, 0x0060, 0x0006, 0x0201, 0x0016, 0x0062, 0x0401, 0x0201, 0x0026,
    0x0054, 0x0201, 0x0045, 0x0063, 0x0a01, 0x0601, 0x0201, 0x0036, 0x0201, 0x0070,
    0x0007, 0x0201, 0x0071, 0x0055, 0x0401, 0x0201, 0x0017, 0x0064, 0x0201, 0x0072,
    0x0027, 0x1801, 0x1001, 0x0801, 0x0401, 0x0201, 0x0046, 0x0073, 0x0201, 0x0037,
    0x0065, 0x0401, 0x0201, 0x0056, 0x0080, 0x0201, 0x0008, 0x0074, 0x0401, 0x0201,
    0x0081, 0x0018, 0x0201, 0x0082, 0x0028, 0x1001, 0x0801, 0x0401, 0x0201, 0x0047,
    0x0066, 0x0201, 0x0083, 0x0038, 0x0401, 0x0201, 0x0075, 0x0057, 0x0201, 0x0084,
    0x0048, 0x0601, 0x0401, 0x0201, 0x0090, 0x0019, 0x0091, 0x0401, 0x0201, 0x0092,
    0x0076, 0x0201, 0x0067, 0x0029, 0x5c01, 0x2401, 0x1201, 0x0a01, 0x0401, 0x0201,
    0x0085, 0x0058, 0x0401, 0x0201, 0x0009, 0x0077, 0x0093, 0x0401, 0x0201, 0x0039,
    0x0094, 0x0201, 0x0049, 0x0086, 0x0a01, 0x0601, 0x0201, 0x0068, 0x0201, 0x00a0,
    0x000a, 0x0201, 0x00a1, 0x001a, 0x0401, 0x0201, 0x00a2, 0x002a, 0x0201, 0x0095,
    0x0059, 0x1a01, 0x0e01, 0x0601, 0x0201, 0x00a3, 0x0201, 0x003a, 0x0087, 0x0401,
    0x0201, 0x0078, 0x00a4, 0x0201, 0x004a, 0x0096, 0x0601, 0x0401, 0x0201, 0x0069,
    0x00b0, 0x00b1, 0x0401, 0x0201, 0x001b, 0x00a5, 0x00b2, 0x0e01, 0x0801, 0x0401,
    0x0201, 0x005a, 0x002b, 0x0201, 0x0088, 0x0097, 0x0201, 0x00b3, 0x0201, 0x0079,
    0x003b, 0x0801, 0x0401, 0x0201, 0x006a, 0x00b4, 0x0201, 0x004b, 0x00c1, 0x0401,
    0x0201, 0x0098, 0x0089, 0x0201, 0x001c, 0x00b5, 0x5001, 0x2201, 0x1001, 0x0601,
    0x0401, 0x0201, 0x005b, 0x002c, 0x00c2, 0x0601, 0x0401, 0x0201, 0x000b, 0x00c0,
    0x00a6, 0x0201, 0x00a7, 0x007a, 0x0a01, 0x0401, 0x0201, 0x00c3, 0x003c, 0x0401,
    0x0201, 0x000c, 0x0099, 0x00b6, 0x0401, 0x0201, 0x006b, 0x00c4, 0x0201, 0x004c,
    0x00a8, 0x1401, 0x0a01, 0x0401, 0x0201, 0x008a, 0x00c5, 0x0401, 0x0201, 0x00d0,
    0x005c, 0x00d1, 0x0401, 0x0201, 0x00b7, 0x007b, 0x0201, 0x001d, 0x0201, 0x000d,
    0x002d, 0x0c01, 0x0401, 0x0201, 0x00d2, 0x00d3, 0x0401, 0x0201, 0x003d, 0x00c6,
    0x0201, 0x006c, 0x00a9, 0x0601, 0x0401, 0x0201, 0x009a, 0x00b8, 0x00d4, 0x0401,
    0x0201, 0x008b, 0x004d, 0x0201, 0x00c7, 0x007c, 0x4401, 0x2201, 0x1201, 0x0a01,
    0x0401, 0x0201, 0x00d5, 0x005d, 0x0401, 0x0201, 0x00e0, 0x000e, 0x00e1, 0x0401,
    0x0201, 0x001e, 0x00e2, 0x0201, 0x00aa, 0x002e, 0x0801, 0x0401, 0x0201, 0x00b9,
    0x009b, 0x0201, 0x00e3, 0x00d6, 0x0401, 0x0201, 0x006d, 0x003e, 0x0201, 0x00c8,
    0x008c, 0x1001, 0x0801, 0x0401, 0x0201, 0x00e4, 0x004e, 0x0201, 0x00d7, 0x007d,
    0x0401, 0x0201, 0x00e5, 0x00ba, 0x0201, 0x00ab, 0x005e, 0x0801, 0x0401, 0x0201,
    0x00c9, 0x009c, 0x0201, 0x00f1, 0x001f, 0x0601, 0x0401, 0x0201, 0x00f0, 0x006e,
    0x00f2, 0x0201, 0x002f, 0x00e6, 0x2601, 0x1201, 0x0801, 0x0401, 0x0201, 0x00d8,
    0x00f3, 0x0201, 0x003f, 0x00f4, 0x0601, 0x0201, 0x004f, 0x0201, 0x008d, 0x00d9,
    0x0201, 0x00bb, 0x00ca, 0x0801, 0x0401, 0x0201, 0x00ac, 0x00e7, 0x0201, 0x007e,
    0x00f5, 0x0801, 0x0401, 0x0201, 0x009d, 0x005f, 0x0201, 0x00e8, 0x008e, 0x0201,
    0x00f6, 0x00cb, 0x2201, 0x1201, 0x0a01, 0x0601, 0x0401, 0x0201, 0x000f, 0x00ae,
    0x006f, 0x0201, 0x00bc, 0x00da, 0x0401, 0x0201, 0x00ad, 0x00f7, 0x0201, 0x007f,
    0x00e9, 0x0801, 0x0401, 0x0201, 0x009e, 0x00cc, 0x0201, 0x00f8, 0x008f, 0x0401,
    0x0201, 0x00db, 0x00bd, 0x0201, 0x00ea, 0x00f9, 0x1001, 0x0801, 0x0401, 0x0201,
    0x009f, 0x00dc, 0x0201, 0x00cd, 0x00eb, 0x0401, 0x0201, 0x00be, 0x00fa, 0x0201,
    0x00af, 0x00dd, 0x0e01, 0x0601, 0x0401, 0x0201, 0x00ec, 0x00ce, 0x00fb, 0x0401,
    0x0201, 0x00bf, 0x00ed, 0x0201, 0x00de, 0x00fc, 0x0601, 0x0401, 0x0201, 0x00cf,
    0x00fd, 0x00ee, 0x0401, 0x0201, 0x00df, 0x00fe, 0x0201, 0x00ef, 0x00ff, 0x0201,
    0x0000, 0x0601, 0x0201, 0x0010, 0x0201, 0x0001, 0x0011, 0x2a01, 0x0801, 0x0401,
    0x0201, 0x0020, 0x0002, 0x0201, 0x0021, 0x0012, 0x0a01, 0x0601, 0x0201, 0x0022,
    0x0201, 0x0030, 0x0003, 0x0201, 0x0031, 0x0013, 0x0a01, 0x0401, 0x0201, 0x0032,
    0x0023, 0x0401, 0x0201, 0x0040, 0x0004, 0x0041, 0x0601, 0x0201, 0x0014, 0x0201,
    0x0033, 0x0042, 0x0401, 0x0201, 0x0024, 0x0050, 0x0201, 0x0043, 0x0034, 0x8a01,
    0x2801, 0x1001, 0x0601, 0x0401, 0x0201, 0x0005, 0x0015, 0x0051, 0x0401, 0x0201,
    0x0052, 0x0025, 0x0401, 0x0201, 0x0044, 0x0035, 0x0053, 0x0a01, 0x0601, 0x0401,
    0x0201, 0x0060, 0x0006, 0x0061, 0x0201, 0x0016, 0x0062, 0x0801, 0x0401, 0x0201,
    0x0026, 0x0054, 0x0201, 0x0045, 0x0063, 0x0401, 0x0201, 0x0036, 0x0070, 0x0071,
    0x2801, 0x1201, 0x0801, 0x0201, 0x0017, 0x0201, 0x0007, 0x0201, 0x0055, 0x0064,
    0x0401, 0x0201, 0x0072, 0x0027, 0x0401, 0x0201, 0x0046, 0x0065, 0x0073, 0x0a01,
    0x0601, 0x0201, 0x0037, 0x0201, 0x0056, 0x0008, 0x0201, 0x0080, 0x0081, 0x0601,
    0x0201, 0x0018, 0x0201, 0x0074, 0x0047, 0x0201, 0x0082, 0x0201, 0x0028, 0x0066,
    0x1801, 0x0e01, 0x0801, 0x0401, 0x0201, 0x0083, 0x0038, 0x0201, 0x0075, 0x0084,
    0x0401, 0x0201, 0x0048, 0x0090, 0x0091, 0x0601, 0x0201, 0x0019, 0x0201, 0x0009,
    0x0076, 0x0201, 0x0092, 0x0029, 0x0e01, 0x0801, 0x0401, 0x0201, 0x0085, 0x0058,
    0x0201, 0x0093, 0x0039, 0x0401, 0x0201, 0x00a0, 0x000a, 0x001a, 0x0801, 0x0201,
    0x00a2, 0x0201, 0x0067, 0x0201, 0x0057, 0x0049, 0x0601, 0x0201, 0x0094, 0x0201,
    0x0077, 0x0086, 0x0201, 0x00a1, 0x0201, 0x0068, 0x0095, 0xdc01, 0x7e01, 0x3201,
    0x1a01, 0x0c01, 0x0601, 0x0201, 0x002a, 0x0201, 0x0059, 0x003a, 0x0201, 0x00a3,
    0x0201, 0x0087, 0x0078, 0x0801, 0x0401, 0x0201, 0x00a4, 0x004a, 0x0201, 0x0096,
    0x0069, 0x0401, 0x0201, 0x00b0, 0x000b, 0x00b1, 0x0a01, 0x0401, 0x0201, 0x001b,
    0x00b2, 0x0201, 0x002b, 0x0201, 0x00a5, 0x005a, 0x0601, 0x0201, 0x00b3, 0x0201,
    0x00a6, 0x006a, 0x0401, 0x0201, 0x00b4, 0x004b, 0x0201, 0x000c, 0x00c1, 0x1e01,
    0x0e01, 0x0601, 0x0401, 0x0201, 0x00b5, 0x00c2, 0x002c, 0x0401, 0x0201, 0x00a7,
    0x00c3, 0x0201, 0x006b, 0x00c4, 0x0801, 0x0201, 0x001d, 0x0401, 0x0201, 0x0088,
    0x0097, 0x003b, 0x0401, 0x0201, 0x00d1, 0x00d2, 0x0201, 0x002d, 0x00d3, 0x1201,
    0x0601, 0x0401, 0x0201, 0x001e, 0x002e, 0x00e2, 0x0601, 0x0401, 0x0201, 0x0079,
    0x0098, 0x00c0, 0x0201, 0x001c, 0x0201, 0x0089, 0x005b, 0x0e01, 0x0601, 0x0201,
    0x003c, 0x0201, 0x007a, 0x00b6, 0x0401, 0x0201, 0x004c, 0x0099, 0x0201, 0x00a8,
    0x008a, 0x0601, 0x0201, 0x000d, 0x0201, 0x00c5, 0x005c, 0x0401, 0x0201, 0x003d,
    0x00c6, 0x0201, 0x006c, 0x009a, 0x5801, 0x5601, 0x2401, 0x1001, 0x0801, 0x0401,
    0x0201, 0x008b, 0x004d, 0x0201, 0x00c7, 0x007c, 0x0401, 0x0201, 0x00d5, 0x005d,
    0x0201, 0x00e0, 0x000e, 0x0801, 0x0201, 0x00e3, 0x0401, 0x0201, 0x00d0, 0x00b7,
    0x007b, 0x0601, 0x0401, 0x0201, 0x00a9, 0x00b8, 0x00d4, 0x0201, 0x00e1, 0x0201,
    0x00aa, 0x00b9, 0x1801, 0x0a01, 0x0601, 0x0401, 0x0201, 0x009b, 0x00d6, 0x006d,
    0x0201, 0x003e, 0x00c8, 0x0601, 0x0401, 0x0201, 0x008c, 0x00e4, 0x004e, 0x0401,
    0x0201, 0x00d7, 0x00e5, 0x0201, 0x00ba, 0x00ab, 0x0c01, 0x0401, 0x0201, 0x009c,
    0x00e6, 0x0401, 0x0201, 0x006e, 0x00d8, 0x0201, 0x008d, 0x00bb, 0x0801, 0x0401,
    0x0201, 0x00e7, 0x009d, 0x0201, 0x00e8, 0x008e, 0x0401, 0x0201, 0x00cb, 0x00bc,
    0x009e, 0x00f1, 0x0201, 0x001f, 0x0201, 0x000f, 0x002f, 0x4201, 0x3801, 0x0201,
    0x00f2, 0x3401, 0x3201, 0x1401, 0x0801, 0x0201, 0x00bd, 0x0201, 0x005e, 0x0201,
    0x007d, 0x00c9, 0x0601, 0x0201, 0x00ca, 0x0201, 0x00ac, 0x007e, 0x0401, 0x0201,
    0x00da, 0x00ad, 0x00cc, 0x0a01, 0x0601, 0x0201, 0x00ae, 0x0201, 0x00db, 0x00dc,
    0x0201, 0x00cd, 0x00be, 0x0601, 0x0401, 0x0201, 0x00eb, 0x00ed, 0x00ee, 0x0601,
    0x0401, 0x0201, 0x00d9, 0x00ea, 0x00e9, 0x0201, 0x00de, 0x0401, 0x0201, 0x00dd,
    0x00ec, 0x00ce, 0x003f, 0x00f0, 0x0401, 0x0201, 0x00f3, 0x00f4, 0x0201, 0x004f,
    0x0201, 0x00f5, 0x005f, 0x0a01, 0x0201, 0x00ff, 0x0401, 0x0201, 0x00f6, 0x006f,
    0x0201, 0x00f7, 0x007f, 0x0c01, 0x0601, 0x0201, 0x008f, 0x0201, 0x00f8, 0x00f9,
    0x0401, 0x0201, 0x009f, 0x00fa, 0x00af, 0x0801, 0x0401, 0x0201, 0x00fb, 0x00bf,
    0x0201, 0x00fc, 0x00cf, 0x0401, 0x0201, 0x00fd, 0x00df, 0x0201, 0x00fe, 0x00ef,
    0x3c01, 0x0801, 0x0401, 0x0201, 0x0000, 0x0010, 0x0201, 0x0001, 0x0011, 0x0e01,
    0x0601, 0x0401, 0x0201, 0x0020, 0x0002, 0x0021, 0x0201, 0x0012, 0x0201, 0x0022,
    0x0201, 0x0030, 0x0003, 0x0e01, 0x0401, 0x0201, 0x0031, 0x0013, 0x0401, 0x0201,
    0x0032, 0x0023, 0x0401, 0x0201, 0x0040, 0x0004, 0x0041, 0x0801, 0x0401, 0x0201,
    0x0014, 0x0033, 0x0201, 0x0042, 0x0024, 0x0601, 0x0401, 0x0201, 0x0043, 0x0034,
    0x0051, 0x0601, 0x0401, 0x0201, 0x0050, 0x0005, 0x0015, 0x0201, 0x0052, 0x0025,
    0xfa01, 0x6201, 0x2201, 0x1201, 0x0a01, 0x0401, 0x0201, 0x0044, 0x0053, 0x0201,
    0x0035, 0x0201, 0x0060, 0x0006, 0x0401, 0x0201, 0x0061, 0x0016, 0x0201, 0x0062,
    0x0026, 0x0801, 0x0401, 0x0201, 0x0054, 0x0045, 0x0201, 0x0063, 0x0036, 0x0401,
    0x0201, 0x0071, 0x0055, 0x0201, 0x0064, 0x0046, 0x2001, 0x0e01, 0x0601, 0x0201,
    0x0072, 0x0201, 0x0027, 0x0037, 0x0201, 0x0073, 0x0401, 0x0201, 0x0070, 0x0007,
    0x0017, 0x0a01, 0x0401, 0x0201, 0x0065, 0x0056, 0x0401, 0x0201, 0x0080, 0x0008,
    0x0081, 0x0401, 0x0201, 0x0074, 0x0047, 0x0201, 0x0018, 0x0082, 0x1001, 0x0801,
    0x0401, 0x0201, 0x0028, 0x0066, 0x0201, 0x0083, 0x0038, 0x0401, 0x0201, 0x0075,
    0x0057, 0x0201, 0x0084, 0x0048, 0x0801, 0x0401, 0x0201, 0x0091, 0x0019, 0x0201,
    0x0092, 0x0076, 0x0401, 0x0201, 0x0067, 0x0029, 0x0201, 0x0085, 0x0058, 0x5c01,
    0x2201, 0x1001, 0x0801, 0x0401, 0x0201, 0x0093, 0x0039, 0x0201, 0x0094, 0x0049,
    0x0401, 0x0201, 0x0077, 0x0086, 0x0201, 0x0068, 0x00a1, 0x0801, 0x0401, 0x0201,
    0x00a2, 0x002a, 0x0201, 0x0095, 0x0059, 0x0401, 0x0201, 0x00a3, 0x003a, 0x0201,
    0x0087, 0x0201, 0x0078, 0x004a, 0x1601, 0x0c01, 0x0401, 0x0201, 0x00a4, 0x0096,
    0x0401, 0x0201, 0x0069, 0x00b1, 0x0201, 0x001b, 0x00a5, 0x0601, 0x0201, 0x00b2,
    0x0201, 0x005a, 0x002b, 0x0201, 0x0088, 0x00b3, 0x1001, 0x0a01, 0x0601, 0x0201,
    0x0090, 0x0201, 0x0009, 0x00a0, 0x0201, 0x0097, 0x0079, 0x0401, 0x0201, 0x00a6,
    0x006a, 0x00b4, 0x0c01, 0x0601, 0x0201, 0x001a, 0x0201, 0x000a, 0x00b0, 0x0201,
    0x003b, 0x0201, 0x000b, 0x00c0, 0x0401, 0x0201, 0x004b, 0x00c1, 0x0201, 0x0098,
    0x0089, 0x4301, 0x2201, 0x1001, 0x0801, 0x0401, 0x0201, 0x001c, 0x00b5, 0x0201,
    0x005b, 0x00c2, 0x0401, 0x0201, 0x002c, 0x00a7, 0x0201, 0x007a, 0x00c3, 0x0a01,
    0x0601, 0x0201, 0x003c, 0x0201, 0x000c, 0x00d0, 0x0201, 0x00b6, 0x006b, 0x0401,
    0x0201, 0x00c4, 0x004c, 0x0201, 0x0099, 0x00a8, 0x1001, 0x0801, 0x0401, 0x0201,
    0x008a, 0x00c5, 0x0201, 0x005c, 0x00d1, 0x0401, 0x0201, 0x00b7, 0x007b, 0x0201,
    0x001d, 0x00d2, 0x0901, 0x0401, 0x0201, 0x002d, 0x00d3, 0x0201, 0x003d, 0x00c6,
    0x55fa, 0x0401, 0x0201, 0x006c, 0x00a9, 0x0201, 0x009a, 0x00d4, 0x2001, 0x1001,
    0x0801, 0x0401, 0x0201, 0x00b8, 0x008b, 0x0201, 0x004d, 0x00c7, 0x0401, 0x0201,
    0x007c, 0x00d5, 0x0201, 0x005d, 0x00e1, 0x0801, 0x0401, 0x0201, 0x001e, 0x00e2,
    0x0201, 0x00aa, 0x00b9, 0x0401, 0x0201, 0x009b, 0x00e3, 0x0201, 0x00d6, 0x006d,
    0x1401, 0x0a01, 0x0601, 0x0201, 0x003e, 0x0201, 0x002e, 0x004e, 0x0201, 0x00c8,
    0x008c, 0x0401, 0x0201, 0x00e4, 0x00d7, 0x0401, 0x0201, 0x007d, 0x00ab, 0x00e5,
    0x0a01, 0x0401, 0x0201, 0x00ba, 0x005e, 0x0201, 0x00c9, 0x0201, 0x009c, 0x006e,
    0x0801, 0x0201, 0x00e6, 0x0201, 0x000d, 0x0201, 0x00e0, 0x000e, 0x0401, 0x0201,
    0x00d8, 0x008d, 0x0201, 0x00bb, 0x00ca, 0x4a01, 0x0201, 0x00ff, 0x4001, 0x3a01,
    0x2001, 0x1001, 0x0801, 0x0401, 0x0201, 0x00ac, 0x00e7, 0x0201, 0x007e, 0x00d9,
    0x0401, 0x0201, 0x009d, 0x00e8, 0x0201, 0x008e, 0x00cb, 0x0801, 0x0401, 0x0201,
    0x00bc, 0x00da, 0x0201, 0x00ad, 0x00e9, 0x0401, 0x0201, 0x009e, 0x00cc, 0x0201,
    0x00db, 0x00bd, 0x1001, 0x0801, 0x0401, 0x0201, 0x00ea, 0x00ae, 0x0201, 0x00dc,
    0x00cd, 0x0401, 0x0201, 0x00eb, 0x00be, 0x0201, 0x00dd, 0x00ec, 0x0801, 0x0401,
    0x0201, 0x00ce, 0x00ed, 0x0201, 0x00de, 0x00ee, 0x000f, 0x0401, 0x0201, 0x00f0,
    0x001f, 0x00f1, 0x0401, 0x0201, 0x00f2, 0x002f, 0x0201, 0x00f3, 0x003f, 0x1201,
    0x0801, 0x0401, 0x0201, 0x00f4, 0x004f, 0x0201, 0x00f5, 0x005f, 0x0401, 0x0201,
    0x00f6, 0x006f, 0x0201, 0x00f7, 0x0201, 0x007f, 0x008f, 0x0a01, 0x0401, 0x0201,
    0x00f8, 0x00f9, 0x0401, 0x0201, 0x009f, 0x00af, 0x00fa, 0x0801, 0x0401, 0x0201,
    0x00fb, 0x00bf, 0x0201, 0x00fc, 0x00cf, 0x0401, 0x0201, 0x00fd, 0x00df, 0x0201,
    0x00fe, 0x00ef, 0x0201, 0x0000, 0x0801, 0x0401, 0x0201, 0x0008, 0x0004, 0x0201,
    0x0001, 0x0002, 0x0801, 0x0401, 0x0201, 0x000c, 0x000a, 0x0201, 0x0003, 0x0006,
    0x0601, 0x0201, 0x0009, 0x0201, 0x0005, 0x0007, 0x0401, 0x0201, 0x000e, 0x000d,
    0x0201, 0x000f, 0x000b, 0x1001, 0x0801, 0x0401, 0x0201, 0x0000, 0x0001, 0x0201,
    0x0002, 0x0003, 0x0401, 0x0201, 0x0004, 0x0005, 0x0201, 0x0006, 0x0007, 0x0801,
    0x0401, 0x0201, 0x0008, 0x0009, 0x0201, 0x000a, 0x000b, 0x0401, 0x0201, 0x000c,
    0x000d, 0x0201, 0x000e, 0x000f,
];

pub(crate) static HUFF_OFFSET: [u16; 34] = [
    0, 0, 7, 24, 0, 41, 72, 103, 174, 245, 316, 443, 570, 697, 0, 1208, 1719, 1719, 1719, 1719, 1719, 1719, 1719, 1719, 2230, 2230, 2230, 2230, 2230, 2230, 2230, 2230, 2742, 2773,
];

pub(crate) static HUFF_LEN: [u16; 34] = [
    0, 7, 17, 17, 0, 31, 31, 71, 71, 71, 127, 127, 127, 511, 0, 511, 511, 511, 511, 511, 511, 511, 511, 511, 512, 512, 512, 512, 512, 512, 512, 512, 31, 31,
];

pub(crate) static HUFF_LINBITS: [u8; 34] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 6, 8, 10, 13, 4, 5, 6, 7, 8, 9, 11, 13, 0, 0,
];

pub(crate) static SYNTH_WINDOW: [f32; 512] = [
    0.0f32, -1.525900006527081e-05f32, -1.525900006527081e-05f32, -1.525900006527081e-05f32, -1.525900006527081e-05f32, -1.525900006527081e-05f32,
    -1.525900006527081e-05f32, -3.051800013054162e-05f32, -3.051800013054162e-05f32, -3.051800013054162e-05f32, -3.051800013054162e-05f32, -4.5775999751640484e-05f32,
    -4.5775999751640484e-05f32, -6.1034999816911295e-05f32, -6.1034999816911295e-05f32, -7.629400352016091e-05f32, -7.629400352016091e-05f32, -9.155300358543172e-05f32,
    -0.00010681199637474492f32, -0.00010681199637474492f32, -0.00012206999963382259f32, -0.0001373289996990934f32, -0.00015258800704032183f32, -0.00016784699982963502f32,
    -0.0001983639958780259f32, -0.00021362300321925431f32, -0.00024414100334979594f32, -0.0002593990066088736f32, -0.0002899169921875f32, -0.00032043500686995685f32,
    -0.0003662109957076609f32, -0.00039672901039011776f32, -0.0004425049992278218f32, -0.0004730219952762127f32, -0.0005340580246411264f32, -0.000579833984375f32,
    -0.0006256100023165345f32, -0.0006866459734737873f32, -0.0007476810133084655f32, -0.0008087159949354827f32, -0.0008850099984556437f32, -0.0009613040019758046f32,
    -0.0010375980054959655f32, -0.0011138919508084655f32, -0.0012054439866915345f32, -0.0012969969538971782f32, -0.0013885500375181437f32, -0.0014801030047237873f32,
    -0.0015869139460846782f32, -0.0016937260515987873f32, -0.0017852779710665345f32, -0.0019073489820584655f32, -0.0020141599234193563f32, -0.0021209719125181437f32,
    -0.0022430419921875f32, -0.0023498539812862873f32, -0.0024566650390625f32, -0.0025787348859012127f32, -0.0026855471078306437f32, -0.0027923579327762127f32,
    -0.002899169921875f32, -0.0029907228890806437f32, -0.0030822749249637127f32, -0.0031738278921693563f32, 0.0032501220703125f32, 0.003326416015625f32,
    0.0033874509390443563f32, 0.0034332280047237873f32, 0.0034637448843568563f32, 0.00347900390625f32, 0.00347900390625f32, 0.0034637448843568563f32,
    0.0034179689828306437f32, 0.0033721919171512127f32, 0.0032806401140987873f32, 0.0031738278921693563f32, 0.0030517580453306437f32, 0.0028839108999818563f32,
    0.0027008058968931437f32, 0.0024871830828487873f32, 0.0022277829702943563f32, 0.0019378659781068563f32, 0.0016174319898709655f32, 0.0012664790265262127f32,
    0.0008697509765625f32, 0.0004425049992278218f32, -3.051800013054162e-05f32, -0.0005493159987963736f32, -0.0010986330453306437f32, -0.0016937260515987873f32,
    -0.0023345949593931437f32, -0.0030059809796512127f32, -0.0037231449969112873f32, -0.004486083984375f32, -0.0052947998046875f32, -0.006118773948401213f32,
    -0.0070037841796875f32, -0.007919312454760075f32, -0.0088653564453125f32, -0.0098419189453125f32, -0.0108489990234375f32, -0.0118865966796875f32,
    -0.012939453125f32, -0.0140228271484375f32, -0.0151214599609375f32, -0.0162353515625f32, -0.0173492431640625f32, -0.018463134765625f32,
    -0.0195770263671875f32, -0.02069091796875f32, -0.02178955078125f32, -0.022857666015625f32, -0.0239105224609375f32, -0.024932861328125f32,
    -0.025909423828125f32, -0.0268402099609375f32, -0.0277252197265625f32, -0.028533935546875f32, -0.0292816162109375f32, -0.029937744140625f32,
    -0.0305328369140625f32, -0.031005859375f32, -0.0313873291015625f32, -0.0316619873046875f32, -0.0318145751953125f32, -0.0318450927734375f32,
    -0.03173828125f32, -0.0314788818359375f32, 0.0310821533203125f32, 0.030517578125f32, 0.02978515625f32, 0.0288848876953125f32,
    0.027801513671875f32, 0.0265350341796875f32, 0.02508544921875f32, 0.0234222412109375f32, 0.021575927734375f32, 0.01953125f32,
    0.0172576904296875f32, 0.014801025390625f32, 0.012115479446947575f32, 0.0092315673828125f32, 0.006134033203125f32, 0.0028228759765625f32,
    -0.0006866459734737873f32, -0.004394530784338713f32, -0.0083160400390625f32, -0.012420654296875f32, -0.0167083740234375f32, -0.02117919921875f32,
    -0.02581787109375f32, -0.030609130859375f32, -0.035552978515625f32, -0.0406341552734375f32, -0.04583740234375f32, -0.0511322021484375f32,
    -0.0565338134765625f32, -0.0619964599609375f32, -0.0675201416015625f32, -0.07305908203125f32, -0.0786285400390625f32, -0.0841827392578125f32,
    -0.0897064208984375f32, -0.0951690673828125f32, -0.1005401611328125f32, -0.1058197021484375f32, -0.1109466552734375f32, -0.1159210205078125f32,
    -0.120697021484375f32, -0.1252593994140625f32, -0.1295623779296875f32, -0.1335906982421875f32, -0.137298583984375f32, -0.1406707763671875f32,
    -0.1436767578125f32, -0.1462554931640625f32, -0.1484222412109375f32, -0.150115966796875f32, -0.15130615234375f32, -0.1519622802734375f32,
    -0.152069091796875f32, -0.1515960693359375f32, -0.1504974365234375f32, -0.148773193359375f32, -0.1463623046875f32, -0.1432647705078125f32,
    -0.1394500732421875f32, -0.1348876953125f32, -0.12957763671875f32, -0.12347412109375f32, -0.1165771484375f32, -0.108856201171875f32,
    0.100311279296875f32, 0.0909271240234375f32, 0.0806884765625f32, 0.0695953369140625f32, 0.0576171875f32, 0.0447845458984375f32,
    0.0310821533203125f32, 0.016510009765625f32, 0.0010681150015443563f32, -0.015228270553052425f32, -0.032379150390625f32, -0.05035400390625f32,
    -0.0691680908203125f32, -0.088775634765625f32, -0.109161376953125f32, -0.13031005859375f32, -0.1522064208984375f32, -0.1747894287109375f32,
    -0.19805908203125f32, -0.22198486328125f32, -0.2465057373046875f32, -0.2715911865234375f32, -0.297210693359375f32, -0.3233184814453125f32,
    -0.3498687744140625f32, -0.376800537109375f32, -0.404083251953125f32, -0.4316558837890625f32, -0.45947265625f32, -0.4874725341796875f32,
    -0.5156097412109375f32, -0.5438232421875f32, -0.5720367431640625f32, -0.6002197265625f32, -0.6282958984375f32, -0.656219482421875f32,
    -0.6839141845703125f32, -0.7113189697265625f32, -0.738372802734375f32, -0.7650299072265625f32, -0.7912139892578125f32, -0.816864013671875f32,
    -0.841949462890625f32, -0.866363525390625f32, -0.8900909423828125f32, -0.913055419921875f32, -0.9351959228515625f32, -0.95648193359375f32,
    -0.9768524169921875f32, -0.996246337890625f32, -1.014617919921875f32, -1.0319366455078125f32, -1.04815673828125f32, -1.0632171630859375f32,
    -1.077117919921875f32, -1.08978271484375f32, -1.1012115478515625f32, -1.1113739013671875f32, -1.1202239990234375f32, -1.12774658203125f32,
    -1.1339263916015625f32, -1.138763427734375f32, -1.1422119140625f32, -1.144287109375f32, 1.144989013671875f32, 1.144287109375f32,
    1.1422119140625f32, 1.138763427734375f32, 1.1339263916015625f32, 1.12774658203125f32, 1.1202239990234375f32, 1.1113739013671875f32,
    1.1012115478515625f32, 1.08978271484375f32, 1.077117919921875f32, 1.0632171630859375f32, 1.04815673828125f32, 1.0319366455078125f32,
    1.014617919921875f32, 0.996246337890625f32, 0.9768524169921875f32, 0.95648193359375f32, 0.9351959228515625f32, 0.913055419921875f32,
    0.8900909423828125f32, 0.866363525390625f32, 0.841949462890625f32, 0.816864013671875f32, 0.7912139892578125f32, 0.7650299072265625f32,
    0.738372802734375f32, 0.7113189697265625f32, 0.6839141845703125f32, 0.656219482421875f32, 0.6282958984375f32, 0.6002197265625f32,
    0.5720367431640625f32, 0.5438232421875f32, 0.5156097412109375f32, 0.4874725341796875f32, 0.45947265625f32, 0.4316558837890625f32,
    0.404083251953125f32, 0.376800537109375f32, 0.3498687744140625f32, 0.3233184814453125f32, 0.297210693359375f32, 0.2715911865234375f32,
    0.2465057373046875f32, 0.22198486328125f32, 0.19805908203125f32, 0.1747894287109375f32, 0.1522064208984375f32, 0.13031005859375f32,
    0.109161376953125f32, 0.088775634765625f32, 0.0691680908203125f32, 0.05035400390625f32, 0.032379150390625f32, 0.015228270553052425f32,
    -0.0010681150015443563f32, -0.016510009765625f32, -0.0310821533203125f32, -0.0447845458984375f32, -0.0576171875f32, -0.0695953369140625f32,
    -0.0806884765625f32, -0.0909271240234375f32, 0.100311279296875f32, 0.108856201171875f32, 0.1165771484375f32, 0.12347412109375f32,
    0.12957763671875f32, 0.1348876953125f32, 0.1394500732421875f32, 0.1432647705078125f32, 0.1463623046875f32, 0.148773193359375f32,
    0.1504974365234375f32, 0.1515960693359375f32, 0.152069091796875f32, 0.1519622802734375f32, 0.15130615234375f32, 0.150115966796875f32,
    0.1484222412109375f32, 0.1462554931640625f32, 0.1436767578125f32, 0.1406707763671875f32, 0.137298583984375f32, 0.1335906982421875f32,
    0.1295623779296875f32, 0.1252593994140625f32, 0.120697021484375f32, 0.1159210205078125f32, 0.1109466552734375f32, 0.1058197021484375f32,
    0.1005401611328125f32, 0.0951690673828125f32, 0.0897064208984375f32, 0.0841827392578125f32, 0.0786285400390625f32, 0.07305908203125f32,
    0.0675201416015625f32, 0.0619964599609375f32, 0.0565338134765625f32, 0.0511322021484375f32, 0.04583740234375f32, 0.0406341552734375f32,
    0.035552978515625f32, 0.030609130859375f32, 0.02581787109375f32, 0.02117919921875f32, 0.0167083740234375f32, 0.012420654296875f32,
    0.0083160400390625f32, 0.004394530784338713f32, 0.0006866459734737873f32, -0.0028228759765625f32, -0.006134033203125f32, -0.0092315673828125f32,
    -0.012115479446947575f32, -0.014801025390625f32, -0.0172576904296875f32, -0.01953125f32, -0.021575927734375f32, -0.0234222412109375f32,
    -0.02508544921875f32, -0.0265350341796875f32, -0.027801513671875f32, -0.0288848876953125f32, -0.02978515625f32, -0.030517578125f32,
    0.0310821533203125f32, 0.0314788818359375f32, 0.03173828125f32, 0.0318450927734375f32, 0.0318145751953125f32, 0.0316619873046875f32,
    0.0313873291015625f32, 0.031005859375f32, 0.0305328369140625f32, 0.029937744140625f32, 0.0292816162109375f32, 0.028533935546875f32,
    0.0277252197265625f32, 0.0268402099609375f32, 0.025909423828125f32, 0.024932861328125f32, 0.0239105224609375f32, 0.022857666015625f32,
    0.02178955078125f32, 0.02069091796875f32, 0.0195770263671875f32, 0.018463134765625f32, 0.0173492431640625f32, 0.0162353515625f32,
    0.0151214599609375f32, 0.0140228271484375f32, 0.012939453125f32, 0.0118865966796875f32, 0.0108489990234375f32, 0.0098419189453125f32,
    0.0088653564453125f32, 0.007919312454760075f32, 0.0070037841796875f32, 0.006118773948401213f32, 0.0052947998046875f32, 0.004486083984375f32,
    0.0037231449969112873f32, 0.0030059809796512127f32, 0.0023345949593931437f32, 0.0016937260515987873f32, 0.0010986330453306437f32, 0.0005493159987963736f32,
    3.051800013054162e-05f32, -0.0004425049992278218f32, -0.0008697509765625f32, -0.0012664790265262127f32, -0.0016174319898709655f32, -0.0019378659781068563f32,
    -0.0022277829702943563f32, -0.0024871830828487873f32, -0.0027008058968931437f32, -0.0028839108999818563f32, -0.0030517580453306437f32, -0.0031738278921693563f32,
    -0.0032806401140987873f32, -0.0033721919171512127f32, -0.0034179689828306437f32, -0.0034637448843568563f32, -0.00347900390625f32, -0.00347900390625f32,
    -0.0034637448843568563f32, -0.0034332280047237873f32, -0.0033874509390443563f32, -0.003326416015625f32, 0.0032501220703125f32, 0.0031738278921693563f32,
    0.0030822749249637127f32, 0.0029907228890806437f32, 0.002899169921875f32, 0.0027923579327762127f32, 0.0026855471078306437f32, 0.0025787348859012127f32,
    0.0024566650390625f32, 0.0023498539812862873f32, 0.0022430419921875f32, 0.0021209719125181437f32, 0.0020141599234193563f32, 0.0019073489820584655f32,
    0.0017852779710665345f32, 0.0016937260515987873f32, 0.0015869139460846782f32, 0.0014801030047237873f32, 0.0013885500375181437f32, 0.0012969969538971782f32,
    0.0012054439866915345f32, 0.0011138919508084655f32, 0.0010375980054959655f32, 0.0009613040019758046f32, 0.0008850099984556437f32, 0.0008087159949354827f32,
    0.0007476810133084655f32, 0.0006866459734737873f32, 0.0006256100023165345f32, 0.000579833984375f32, 0.0005340580246411264f32, 0.0004730219952762127f32,
    0.0004425049992278218f32, 0.00039672901039011776f32, 0.0003662109957076609f32, 0.00032043500686995685f32, 0.0002899169921875f32, 0.0002593990066088736f32,
    0.00024414100334979594f32, 0.00021362300321925431f32, 0.0001983639958780259f32, 0.00016784699982963502f32, 0.00015258800704032183f32, 0.0001373289996990934f32,
    0.00012206999963382259f32, 0.00010681199637474492f32, 0.00010681199637474492f32, 9.155300358543172e-05f32, 7.629400352016091e-05f32, 7.629400352016091e-05f32,
    6.1034999816911295e-05f32, 6.1034999816911295e-05f32, 4.5775999751640484e-05f32, 4.5775999751640484e-05f32, 3.051800013054162e-05f32, 3.051800013054162e-05f32,
    3.051800013054162e-05f32, 3.051800013054162e-05f32, 1.525900006527081e-05f32, 1.525900006527081e-05f32, 1.525900006527081e-05f32, 1.525900006527081e-05f32,
    1.525900006527081e-05f32, 1.525900006527081e-05f32,
];

pub(crate) static SYNTH_COS: [[f32; 32]; 64] = [
    [0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32],
    [0.6715589761734009f32, -0.803207516670227f32, -0.5141027569770813f32, 0.903989315032959f32, 0.3368898630142212f32, -0.9700312614440918f32, -0.1467304676771164f32, 0.9987954497337341f32, -0.049067676067352295f32, -0.9891765117645264f32, 0.24298018217086792f32, 0.9415440559387207f32, -0.4275550842285156f32, -0.8577286005020142f32, 0.5956993103027344f32, 0.7409511208534241f32, -0.7409511208534241f32, -0.5956993103027344f32, 0.8577286005020142f32, 0.4275550842285156f32, -0.9415440559387207f32, -0.24298018217086792f32, 0.9891765117645264f32, 0.049067676067352295f32, -0.9987954497337341f32, 0.1467304676771164f32, 0.9700312614440918f32, -0.3368898630142212f32, -0.903989315032959f32, 0.5141027569770813f32, 0.803207516670227f32, -0.6715589761734009f32],
    [0.6343932747840881f32, -0.8819212913513184f32, -0.290284663438797f32, 0.9951847195625305f32, -0.0980171412229538f32, -0.9569403529167175f32, 0.4713967442512512f32, 0.7730104327201843f32, -0.7730104327201843f32, -0.4713967442512512f32, 0.9569403529167175f32, 0.0980171412229538f32, -0.9951847195625305f32, 0.290284663438797f32, 0.8819212913513184f32, -0.6343932747840881f32, -0.6343932747840881f32, 0.8819212913513184f32, 0.290284663438797f32, -0.9951847195625305f32, 0.0980171412229538f32, 0.9569403529167175f32, -0.4713967442512512f32, -0.7730104327201843f32, 0.7730104327201843f32, 0.4713967442512512f32, -0.9569403529167175f32, -0.0980171412229538f32, 0.9951847195625305f32, -0.290284663438797f32, -0.8819212913513184f32, 0.6343932747840881f32],
    [0.5956993103027344f32, -0.9415440559387207f32, -0.049067676067352295f32, 0.9700312614440918f32, -0.5141027569770813f32, -0.6715589761734009f32, 0.903989315032959f32, 0.1467304676771164f32, -0.9891765117645264f32, 0.4275550842285156f32, 0.7409511208534241f32, -0.8577286005020142f32, -0.24298018217086792f32, 0.9987954497337341f32, -0.3368898630142212f32, -0.803207516670227f32, 0.803207516670227f32, 0.3368898630142212f32, -0.9987954497337341f32, 0.24298018217086792f32, 0.8577286005020142f32, -0.7409511208534241f32, -0.4275550842285156f32, 0.9891765117645264f32, -0.1467304676771164f32, -0.903989315032959f32, 0.6715589761734009f32, 0.5141027569770813f32, -0.9700312614440918f32, 0.049067676067352295f32, 0.9415440559387207f32, -0.5956993103027344f32],
    [0.5555702447891235f32, -0.9807852506637573f32, 0.19509032368659973f32, 0.8314695954322815f32, -0.8314695954322815f32, -0.19509032368659973f32, 0.9807852506637573f32, -0.5555702447891235f32, -0.5555702447891235f32, 0.9807852506637573f32, -0.19509032368659973f32, -0.8314695954322815f32, 0.8314695954322815f32, 0.19509032368659973f32, -0.9807852506637573f32, 0.5555702447891235f32, 0.5555702447891235f32, -0.9807852506637573f32, 0.19509032368659973f32, 0.8314695954322815f32, -0.8314695954322815f32, -0.19509032368659973f32, 0.9807852506637573f32, -0.5555702447891235f32, -0.5555702447891235f32, 0.9807852506637573f32, -0.19509032368659973f32, -0.8314695954322815f32, 0.8314695954322815f32, 0.19509032368659973f32, -0.9807852506637573f32, 0.5555702447891235f32],
    [0.5141027569770813f32, -0.9987954497337341f32, 0.4275550842285156f32, 0.5956993103027344f32, -0.9891765117645264f32, 0.3368898630142212f32, 0.6715589761734009f32, -0.9700312614440918f32, 0.24298018217086792f32, 0.7409511208534241f32, -0.9415440559387207f32, 0.1467304676771164f32, 0.803207516670227f32, -0.903989315032959f32, 0.049067676067352295f32, 0.8577286005020142f32, -0.8577286005020142f32, -0.049067676067352295f32, 0.903989315032959f32, -0.803207516670227f32, -0.1467304676771164f32, 0.9415440559387207f32, -0.7409511208534241f32, -0.24298018217086792f32, 0.9700312614440918f32, -0.6715589761734009f32, -0.3368898630142212f32, 0.9891765117645264f32, -0.5956993103027344f32, -0.4275550842285156f32, 0.9987954497337341f32, -0.5141027569770813f32],
    [0.4713967442512512f32, -0.9951847195625305f32, 0.6343932747840881f32, 0.290284663438797f32, -0.9569403529167175f32, 0.7730104327201843f32, 0.0980171412229538f32, -0.8819212913513184f32, 0.8819212913513184f32, -0.0980171412229538f32, -0.7730104327201843f32, 0.9569403529167175f32, -0.290284663438797f32, -0.6343932747840881f32, 0.9951847195625305f32, -0.4713967442512512f32, -0.4713967442512512f32, 0.9951847195625305f32, -0.6343932747840881f32, -0.290284663438797f32, 0.9569403529167175f32, -0.7730104327201843f32, -0.0980171412229538f32, 0.8819212913513184f32, -0.8819212913513184f32, 0.0980171412229538f32, 0.7730104327201843f32, -0.9569403529167175f32, 0.290284663438797f32, 0.6343932747840881f32, -0.9951847195625305f32, 0.4713967442512512f32],
    [0.4275550842285156f32, -0.9700312614440918f32, 0.803207516670227f32, -0.049067676067352295f32, -0.7409511208534241f32, 0.9891765117645264f32, -0.5141027569770813f32, -0.3368898630142212f32, 0.9415440559387207f32, -0.8577286005020142f32, 0.1467304676771164f32, 0.6715589761734009f32, -0.9987954497337341f32, 0.5956993103027344f32, 0.24298018217086792f32, -0.903989315032959f32, 0.903989315032959f32, -0.24298018217086792f32, -0.5956993103027344f32, 0.9987954497337341f32, -0.6715589761734009f32, -0.1467304676771164f32, 0.8577286005020142f32, -0.9415440559387207f32, 0.3368898630142212f32, 0.5141027569770813f32, -0.9891765117645264f32, 0.7409511208534241f32, 0.049067676067352295f32, -0.803207516670227f32, 0.9700312614440918f32, -0.4275550842285156f32],
    [0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32],
    [0.3368898630142212f32, -0.8577286005020142f32, 0.9891765117645264f32, -0.6715589761734009f32, 0.049067676067352295f32, 0.5956993103027344f32, -0.9700312614440918f32, 0.903989315032959f32, -0.4275550842285156f32, -0.24298018217086792f32, 0.803207516670227f32, -0.9987954497337341f32, 0.7409511208534241f32, -0.1467304676771164f32, -0.5141027569770813f32, 0.9415440559387207f32, -0.9415440559387207f32, 0.5141027569770813f32, 0.1467304676771164f32, -0.7409511208534241f32, 0.9987954497337341f32, -0.803207516670227f32, 0.24298018217086792f32, 0.4275550842285156f32, -0.903989315032959f32, 0.9700312614440918f32, -0.5956993103027344f32, -0.049067676067352295f32, 0.6715589761734009f32, -0.9891765117645264f32, 0.8577286005020142f32, -0.3368898630142212f32],
    [0.290284663438797f32, -0.7730104327201843f32, 0.9951847195625305f32, -0.8819212913513184f32, 0.4713967442512512f32, 0.0980171412229538f32, -0.6343932747840881f32, 0.9569403529167175f32, -0.9569403529167175f32, 0.6343932747840881f32, -0.0980171412229538f32, -0.4713967442512512f32, 0.8819212913513184f32, -0.9951847195625305f32, 0.7730104327201843f32, -0.290284663438797f32, -0.290284663438797f32, 0.7730104327201843f32, -0.9951847195625305f32, 0.8819212913513184f32, -0.4713967442512512f32, -0.0980171412229538f32, 0.6343932747840881f32, -0.9569403529167175f32, 0.9569403529167175f32, -0.6343932747840881f32, 0.0980171412229538f32, 0.4713967442512512f32, -0.8819212913513184f32, 0.9951847195625305f32, -0.7730104327201843f32, 0.290284663438797f32],
    [0.24298018217086792f32, -0.6715589761734009f32, 0.9415440559387207f32, -0.9891765117645264f32, 0.803207516670227f32, -0.4275550842285156f32, -0.049067676067352295f32, 0.5141027569770813f32, -0.8577286005020142f32, 0.9987954497337341f32, -0.903989315032959f32, 0.5956993103027344f32, -0.1467304676771164f32, -0.3368898630142212f32, 0.7409511208534241f32, -0.9700312614440918f32, 0.9700312614440918f32, -0.7409511208534241f32, 0.3368898630142212f32, 0.1467304676771164f32, -0.5956993103027344f32, 0.903989315032959f32, -0.9987954497337341f32, 0.8577286005020142f32, -0.5141027569770813f32, 0.049067676067352295f32, 0.4275550842285156f32, -0.803207516670227f32, 0.9891765117645264f32, -0.9415440559387207f32, 0.6715589761734009f32, -0.24298018217086792f32],
    [0.19509032368659973f32, -0.5555702447891235f32, 0.8314695954322815f32, -0.9807852506637573f32, 0.9807852506637573f32, -0.8314695954322815f32, 0.5555702447891235f32, -0.19509032368659973f32, -0.19509032368659973f32, 0.5555702447891235f32, -0.8314695954322815f32, 0.9807852506637573f32, -0.9807852506637573f32, 0.8314695954322815f32, -0.5555702447891235f32, 0.19509032368659973f32, 0.19509032368659973f32, -0.5555702447891235f32, 0.8314695954322815f32, -0.9807852506637573f32, 0.9807852506637573f32, -0.8314695954322815f32, 0.5555702447891235f32, -0.19509032368659973f32, -0.19509032368659973f32, 0.5555702447891235f32, -0.8314695954322815f32, 0.9807852506637573f32, -0.9807852506637573f32, 0.8314695954322815f32, -0.5555702447891235f32, 0.19509032368659973f32],
    [0.1467304676771164f32, -0.4275550842285156f32, 0.6715589761734009f32, -0.8577286005020142f32, 0.9700312614440918f32, -0.9987954497337341f32, 0.9415440559387207f32, -0.803207516670227f32, 0.5956993103027344f32, -0.3368898630142212f32, 0.049067676067352295f32, 0.24298018217086792f32, -0.5141027569770813f32, 0.7409511208534241f32, -0.903989315032959f32, 0.9891765117645264f32, -0.9891765117645264f32, 0.903989315032959f32, -0.7409511208534241f32, 0.5141027569770813f32, -0.24298018217086792f32, -0.049067676067352295f32, 0.3368898630142212f32, -0.5956993103027344f32, 0.803207516670227f32, -0.9415440559387207f32, 0.9987954497337341f32, -0.9700312614440918f32, 0.8577286005020142f32, -0.6715589761734009f32, 0.4275550842285156f32, -0.1467304676771164f32],
    [0.0980171412229538f32, -0.290284663438797f32, 0.4713967442512512f32, -0.6343932747840881f32, 0.7730104327201843f32, -0.8819212913513184f32, 0.9569403529167175f32, -0.9951847195625305f32, 0.9951847195625305f32, -0.9569403529167175f32, 0.8819212913513184f32, -0.7730104327201843f32, 0.6343932747840881f32, -0.4713967442512512f32, 0.290284663438797f32, -0.0980171412229538f32, -0.0980171412229538f32, 0.290284663438797f32, -0.4713967442512512f32, 0.6343932747840881f32, -0.7730104327201843f32, 0.8819212913513184f32, -0.9569403529167175f32, 0.9951847195625305f32, -0.9951847195625305f32, 0.9569403529167175f32, -0.8819212913513184f32, 0.7730104327201843f32, -0.6343932747840881f32, 0.4713967442512512f32, -0.290284663438797f32, 0.0980171412229538f32],
    [0.049067676067352295f32, -0.1467304676771164f32, 0.24298018217086792f32, -0.3368898630142212f32, 0.4275550842285156f32, -0.5141027569770813f32, 0.5956993103027344f32, -0.6715589761734009f32, 0.7409511208534241f32, -0.803207516670227f32, 0.8577286005020142f32, -0.903989315032959f32, 0.9415440559387207f32, -0.9700312614440918f32, 0.9891765117645264f32, -0.9987954497337341f32, 0.9987954497337341f32, -0.9891765117645264f32, 0.9700312614440918f32, -0.9415440559387207f32, 0.903989315032959f32, -0.8577286005020142f32, 0.803207516670227f32, -0.7409511208534241f32, 0.6715589761734009f32, -0.5956993103027344f32, 0.5141027569770813f32, -0.4275550842285156f32, 0.3368898630142212f32, -0.24298018217086792f32, 0.1467304676771164f32, -0.049067676067352295f32],
    [6.123234262925839e-17f32, -1.8369701465288538e-16f32, 3.0616169991140216e-16f32, -4.2862638516991895e-16f32, 5.510910704284357e-16f32, -2.4499125686174234e-15f32, -9.803364513943404e-16f32, -2.6948418861948977e-15f32, -7.354070808773068e-16f32, -2.939771203772372e-15f32, -4.904777103602732e-16f32, -3.1847007331080833e-15f32, -2.4554833984323965e-16f32, -3.4296300506855577e-15f32, -6.189806482407143e-19f32, -3.674559368263032e-15f32, 2.443103747210479e-16f32, -3.9194886858405064e-15f32, 4.892397187683019e-16f32, -4.164418003417981e-15f32, 7.839596552765456e-15f32, -4.409347320995455e-15f32, 9.790984068628098e-16f32, 2.4511505072698355e-15f32, 8.329455187920404e-15f32, -4.8992063796668775e-15f32, 1.468957147896877e-15f32, 1.961291872114887e-15f32, 8.819313823075353e-15f32, -5.389065014821826e-15f32, 1.9588159948100625e-15f32, 1.4714331310808197e-15f32],
    [-0.049067676067352295f32, 0.1467304676771164f32, -0.24298018217086792f32, 0.3368898630142212f32, -0.4275550842285156f32, 0.5141027569770813f32, -0.5956993103027344f32, 0.6715589761734009f32, -0.7409511208534241f32, 0.803207516670227f32, -0.8577286005020142f32, 0.903989315032959f32, -0.9415440559387207f32, 0.9700312614440918f32, -0.9891765117645264f32, 0.9987954497337341f32, -0.9987954497337341f32, 0.9891765117645264f32, -0.9700312614440918f32, 0.9415440559387207f32, -0.903989315032959f32, 0.8577286005020142f32, -0.803207516670227f32, 0.7409511208534241f32, -0.6715589761734009f32, 0.5956993103027344f32, -0.5141027569770813f32, 0.4275550842285156f32, -0.3368898630142212f32, 0.24298018217086792f32, -0.1467304676771164f32, 0.049067676067352295f32],
    [-0.0980171412229538f32, 0.290284663438797f32, -0.4713967442512512f32, 0.6343932747840881f32, -0.7730104327201843f32, 0.8819212913513184f32, -0.9569403529167175f32, 0.9951847195625305f32, -0.9951847195625305f32, 0.9569403529167175f32, -0.8819212913513184f32, 0.7730104327201843f32, -0.6343932747840881f32, 0.4713967442512512f32, -0.290284663438797f32, 0.0980171412229538f32, 0.0980171412229538f32, -0.290284663438797f32, 0.4713967442512512f32, -0.6343932747840881f32, 0.7730104327201843f32, -0.8819212913513184f32, 0.9569403529167175f32, -0.9951847195625305f32, 0.9951847195625305f32, -0.9569403529167175f32, 0.8819212913513184f32, -0.7730104327201843f32, 0.6343932747840881f32, -0.4713967442512512f32, 0.290284663438797f32, -0.0980171412229538f32],
    [-0.1467304676771164f32, 0.4275550842285156f32, -0.6715589761734009f32, 0.8577286005020142f32, -0.9700312614440918f32, 0.9987954497337341f32, -0.9415440559387207f32, 0.803207516670227f32, -0.5956993103027344f32, 0.3368898630142212f32, -0.049067676067352295f32, -0.24298018217086792f32, 0.5141027569770813f32, -0.7409511208534241f32, 0.903989315032959f32, -0.9891765117645264f32, 0.9891765117645264f32, -0.903989315032959f32, 0.7409511208534241f32, -0.5141027569770813f32, 0.24298018217086792f32, 0.049067676067352295f32, -0.3368898630142212f32, 0.5956993103027344f32, -0.803207516670227f32, 0.9415440559387207f32, -0.9987954497337341f32, 0.9700312614440918f32, -0.8577286005020142f32, 0.6715589761734009f32, -0.4275550842285156f32, 0.1467304676771164f32],
    [-0.19509032368659973f32, 0.5555702447891235f32, -0.8314695954322815f32, 0.9807852506637573f32, -0.9807852506637573f32, 0.8314695954322815f32, -0.5555702447891235f32, 0.19509032368659973f32, 0.19509032368659973f32, -0.5555702447891235f32, 0.8314695954322815f32, -0.9807852506637573f32, 0.9807852506637573f32, -0.8314695954322815f32, 0.5555702447891235f32, -0.19509032368659973f32, -0.19509032368659973f32, 0.5555702447891235f32, -0.8314695954322815f32, 0.9807852506637573f32, -0.9807852506637573f32, 0.8314695954322815f32, -0.5555702447891235f32, 0.19509032368659973f32, 0.19509032368659973f32, -0.5555702447891235f32, 0.8314695954322815f32, -0.9807852506637573f32, 0.9807852506637573f32, -0.8314695954322815f32, 0.5555702447891235f32, -0.19509032368659973f32],
    [-0.24298018217086792f32, 0.6715589761734009f32, -0.9415440559387207f32, 0.9891765117645264f32, -0.803207516670227f32, 0.4275550842285156f32, 0.049067676067352295f32, -0.5141027569770813f32, 0.8577286005020142f32, -0.9987954497337341f32, 0.903989315032959f32, -0.5956993103027344f32, 0.1467304676771164f32, 0.3368898630142212f32, -0.7409511208534241f32, 0.9700312614440918f32, -0.9700312614440918f32, 0.7409511208534241f32, -0.3368898630142212f32, -0.1467304676771164f32, 0.5956993103027344f32, -0.903989315032959f32, 0.9987954497337341f32, -0.8577286005020142f32, 0.5141027569770813f32, -0.049067676067352295f32, -0.4275550842285156f32, 0.803207516670227f32, -0.9891765117645264f32, 0.9415440559387207f32, -0.6715589761734009f32, 0.24298018217086792f32],
    [-0.290284663438797f32, 0.7730104327201843f32, -0.9951847195625305f32, 0.8819212913513184f32, -0.4713967442512512f32, -0.0980171412229538f32, 0.6343932747840881f32, -0.9569403529167175f32, 0.9569403529167175f32, -0.6343932747840881f32, 0.0980171412229538f32, 0.4713967442512512f32, -0.8819212913513184f32, 0.9951847195625305f32, -0.7730104327201843f32, 0.290284663438797f32, 0.290284663438797f32, -0.7730104327201843f32, 0.9951847195625305f32, -0.8819212913513184f32, 0.4713967442512512f32, 0.0980171412229538f32, -0.6343932747840881f32, 0.9569403529167175f32, -0.9569403529167175f32, 0.6343932747840881f32, -0.0980171412229538f32, -0.4713967442512512f32, 0.8819212913513184f32, -0.9951847195625305f32, 0.7730104327201843f32, -0.290284663438797f32],
    [-0.3368898630142212f32, 0.8577286005020142f32, -0.9891765117645264f32, 0.6715589761734009f32, -0.049067676067352295f32, -0.5956993103027344f32, 0.9700312614440918f32, -0.903989315032959f32, 0.4275550842285156f32, 0.24298018217086792f32, -0.803207516670227f32, 0.9987954497337341f32, -0.7409511208534241f32, 0.1467304676771164f32, 0.5141027569770813f32, -0.9415440559387207f32, 0.9415440559387207f32, -0.5141027569770813f32, -0.1467304676771164f32, 0.7409511208534241f32, -0.9987954497337341f32, 0.803207516670227f32, -0.24298018217086792f32, -0.4275550842285156f32, 0.903989315032959f32, -0.9700312614440918f32, 0.5956993103027344f32, 0.049067676067352295f32, -0.6715589761734009f32, 0.9891765117645264f32, -0.8577286005020142f32, 0.3368898630142212f32],
    [-0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32],
    [-0.4275550842285156f32, 0.9700312614440918f32, -0.803207516670227f32, 0.049067676067352295f32, 0.7409511208534241f32, -0.9891765117645264f32, 0.5141027569770813f32, 0.3368898630142212f32, -0.9415440559387207f32, 0.8577286005020142f32, -0.1467304676771164f32, -0.6715589761734009f32, 0.9987954497337341f32, -0.5956993103027344f32, -0.24298018217086792f32, 0.903989315032959f32, -0.903989315032959f32, 0.24298018217086792f32, 0.5956993103027344f32, -0.9987954497337341f32, 0.6715589761734009f32, 0.1467304676771164f32, -0.8577286005020142f32, 0.9415440559387207f32, -0.3368898630142212f32, -0.5141027569770813f32, 0.9891765117645264f32, -0.7409511208534241f32, -0.049067676067352295f32, 0.803207516670227f32, -0.9700312614440918f32, 0.4275550842285156f32],
    [-0.4713967442512512f32, 0.9951847195625305f32, -0.6343932747840881f32, -0.290284663438797f32, 0.9569403529167175f32, -0.7730104327201843f32, -0.0980171412229538f32, 0.8819212913513184f32, -0.8819212913513184f32, 0.0980171412229538f32, 0.7730104327201843f32, -0.9569403529167175f32, 0.290284663438797f32, 0.6343932747840881f32, -0.9951847195625305f32, 0.4713967442512512f32, 0.4713967442512512f32, -0.9951847195625305f32, 0.6343932747840881f32, 0.290284663438797f32, -0.9569403529167175f32, 0.7730104327201843f32, 0.0980171412229538f32, -0.8819212913513184f32, 0.8819212913513184f32, -0.0980171412229538f32, -0.7730104327201843f32, 0.9569403529167175f32, -0.290284663438797f32, -0.6343932747840881f32, 0.9951847195625305f32, -0.4713967442512512f32],
    [-0.5141027569770813f32, 0.9987954497337341f32, -0.4275550842285156f32, -0.5956993103027344f32, 0.9891765117645264f32, -0.3368898630142212f32, -0.6715589761734009f32, 0.9700312614440918f32, -0.24298018217086792f32, -0.7409511208534241f32, 0.9415440559387207f32, -0.1467304676771164f32, -0.803207516670227f32, 0.903989315032959f32, -0.049067676067352295f32, -0.8577286005020142f32, 0.8577286005020142f32, 0.049067676067352295f32, -0.903989315032959f32, 0.803207516670227f32, 0.1467304676771164f32, -0.9415440559387207f32, 0.7409511208534241f32, 0.24298018217086792f32, -0.9700312614440918f32, 0.6715589761734009f32, 0.3368898630142212f32, -0.9891765117645264f32, 0.5956993103027344f32, 0.4275550842285156f32, -0.9987954497337341f32, 0.5141027569770813f32],
    [-0.5555702447891235f32, 0.9807852506637573f32, -0.19509032368659973f32, -0.8314695954322815f32, 0.8314695954322815f32, 0.19509032368659973f32, -0.9807852506637573f32, 0.5555702447891235f32, 0.5555702447891235f32, -0.9807852506637573f32, 0.19509032368659973f32, 0.8314695954322815f32, -0.8314695954322815f32, -0.19509032368659973f32, 0.9807852506637573f32, -0.5555702447891235f32, -0.5555702447891235f32, 0.9807852506637573f32, -0.19509032368659973f32, -0.8314695954322815f32, 0.8314695954322815f32, 0.19509032368659973f32, -0.9807852506637573f32, 0.5555702447891235f32, 0.5555702447891235f32, -0.9807852506637573f32, 0.19509032368659973f32, 0.8314695954322815f32, -0.8314695954322815f32, -0.19509032368659973f32, 0.9807852506637573f32, -0.5555702447891235f32],
    [-0.5956993103027344f32, 0.9415440559387207f32, 0.049067676067352295f32, -0.9700312614440918f32, 0.5141027569770813f32, 0.6715589761734009f32, -0.903989315032959f32, -0.1467304676771164f32, 0.9891765117645264f32, -0.4275550842285156f32, -0.7409511208534241f32, 0.8577286005020142f32, 0.24298018217086792f32, -0.9987954497337341f32, 0.3368898630142212f32, 0.803207516670227f32, -0.803207516670227f32, -0.3368898630142212f32, 0.9987954497337341f32, -0.24298018217086792f32, -0.8577286005020142f32, 0.7409511208534241f32, 0.4275550842285156f32, -0.9891765117645264f32, 0.1467304676771164f32, 0.903989315032959f32, -0.6715589761734009f32, -0.5141027569770813f32, 0.9700312614440918f32, -0.049067676067352295f32, -0.9415440559387207f32, 0.5956993103027344f32],
    [-0.6343932747840881f32, 0.8819212913513184f32, 0.290284663438797f32, -0.9951847195625305f32, 0.0980171412229538f32, 0.9569403529167175f32, -0.4713967442512512f32, -0.7730104327201843f32, 0.7730104327201843f32, 0.4713967442512512f32, -0.9569403529167175f32, -0.0980171412229538f32, 0.9951847195625305f32, -0.290284663438797f32, -0.8819212913513184f32, 0.6343932747840881f32, 0.6343932747840881f32, -0.8819212913513184f32, -0.290284663438797f32, 0.9951847195625305f32, -0.0980171412229538f32, -0.9569403529167175f32, 0.4713967442512512f32, 0.7730104327201843f32, -0.7730104327201843f32, -0.4713967442512512f32, 0.9569403529167175f32, 0.0980171412229538f32, -0.9951847195625305f32, 0.290284663438797f32, 0.8819212913513184f32, -0.6343932747840881f32],
    [-0.6715589761734009f32, 0.803207516670227f32, 0.5141027569770813f32, -0.903989315032959f32, -0.3368898630142212f32, 0.9700312614440918f32, 0.1467304676771164f32, -0.9987954497337341f32, 0.049067676067352295f32, 0.9891765117645264f32, -0.24298018217086792f32, -0.9415440559387207f32, 0.4275550842285156f32, 0.8577286005020142f32, -0.5956993103027344f32, -0.7409511208534241f32, 0.7409511208534241f32, 0.5956993103027344f32, -0.8577286005020142f32, -0.4275550842285156f32, 0.9415440559387207f32, 0.24298018217086792f32, -0.9891765117645264f32, -0.049067676067352295f32, 0.9987954497337341f32, -0.1467304676771164f32, -0.9700312614440918f32, 0.3368898630142212f32, 0.903989315032959f32, -0.5141027569770813f32, -0.803207516670227f32, 0.6715589761734009f32],
    [-0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32, -0.7071067690849304f32, 0.7071067690849304f32, 0.7071067690849304f32, -0.7071067690849304f32],
    [-0.7409511208534241f32, 0.5956993103027344f32, 0.8577286005020142f32, -0.4275550842285156f32, -0.9415440559387207f32, 0.24298018217086792f32, 0.9891765117645264f32, -0.049067676067352295f32, -0.9987954497337341f32, -0.1467304676771164f32, 0.9700312614440918f32, 0.3368898630142212f32, -0.903989315032959f32, -0.5141027569770813f32, 0.803207516670227f32, 0.6715589761734009f32, -0.6715589761734009f32, -0.803207516670227f32, 0.5141027569770813f32, 0.903989315032959f32, -0.3368898630142212f32, -0.9700312614440918f32, 0.1467304676771164f32, 0.9987954497337341f32, 0.049067676067352295f32, -0.9891765117645264f32, -0.24298018217086792f32, 0.9415440559387207f32, 0.4275550842285156f32, -0.8577286005020142f32, -0.5956993103027344f32, 0.7409511208534241f32],
    [-0.7730104327201843f32, 0.4713967442512512f32, 0.9569403529167175f32, -0.0980171412229538f32, -0.9951847195625305f32, -0.290284663438797f32, 0.8819212913513184f32, 0.6343932747840881f32, -0.6343932747840881f32, -0.8819212913513184f32, 0.290284663438797f32, 0.9951847195625305f32, 0.0980171412229538f32, -0.9569403529167175f32, -0.4713967442512512f32, 0.7730104327201843f32, 0.7730104327201843f32, -0.4713967442512512f32, -0.9569403529167175f32, 0.0980171412229538f32, 0.9951847195625305f32, 0.290284663438797f32, -0.8819212913513184f32, -0.6343932747840881f32, 0.6343932747840881f32, 0.8819212913513184f32, -0.290284663438797f32, -0.9951847195625305f32, -0.0980171412229538f32, 0.9569403529167175f32, 0.4713967442512512f32, -0.7730104327201843f32],
    [-0.803207516670227f32, 0.3368898630142212f32, 0.9987954497337341f32, 0.24298018217086792f32, -0.8577286005020142f32, -0.7409511208534241f32, 0.4275550842285156f32, 0.9891765117645264f32, 0.1467304676771164f32, -0.903989315032959f32, -0.6715589761734009f32, 0.5141027569770813f32, 0.9700312614440918f32, 0.049067676067352295f32, -0.9415440559387207f32, -0.5956993103027344f32, 0.5956993103027344f32, 0.9415440559387207f32, -0.049067676067352295f32, -0.9700312614440918f32, -0.5141027569770813f32, 0.6715589761734009f32, 0.903989315032959f32, -0.1467304676771164f32, -0.9891765117645264f32, -0.4275550842285156f32, 0.7409511208534241f32, 0.8577286005020142f32, -0.24298018217086792f32, -0.9987954497337341f32, -0.3368898630142212f32, 0.803207516670227f32],
    [-0.8314695954322815f32, 0.19509032368659973f32, 0.9807852506637573f32, 0.5555702447891235f32, -0.5555702447891235f32, -0.9807852506637573f32, -0.19509032368659973f32, 0.8314695954322815f32, 0.8314695954322815f32, -0.19509032368659973f32, -0.9807852506637573f32, -0.5555702447891235f32, 0.5555702447891235f32, 0.9807852506637573f32, 0.19509032368659973f32, -0.8314695954322815f32, -0.8314695954322815f32, 0.19509032368659973f32, 0.9807852506637573f32, 0.5555702447891235f32, -0.5555702447891235f32, -0.9807852506637573f32, -0.19509032368659973f32, 0.8314695954322815f32, 0.8314695954322815f32, -0.19509032368659973f32, -0.9807852506637573f32, -0.5555702447891235f32, 0.5555702447891235f32, 0.9807852506637573f32, 0.19509032368659973f32, -0.8314695954322815f32],
    [-0.8577286005020142f32, 0.049067676067352295f32, 0.903989315032959f32, 0.803207516670227f32, -0.1467304676771164f32, -0.9415440559387207f32, -0.7409511208534241f32, 0.24298018217086792f32, 0.9700312614440918f32, 0.6715589761734009f32, -0.3368898630142212f32, -0.9891765117645264f32, -0.5956993103027344f32, 0.4275550842285156f32, 0.9987954497337341f32, 0.5141027569770813f32, -0.5141027569770813f32, -0.9987954497337341f32, -0.4275550842285156f32, 0.5956993103027344f32, 0.9891765117645264f32, 0.3368898630142212f32, -0.6715589761734009f32, -0.9700312614440918f32, -0.24298018217086792f32, 0.7409511208534241f32, 0.9415440559387207f32, 0.1467304676771164f32, -0.803207516670227f32, -0.903989315032959f32, -0.049067676067352295f32, 0.8577286005020142f32],
    [-0.8819212913513184f32, -0.0980171412229538f32, 0.7730104327201843f32, 0.9569403529167175f32, 0.290284663438797f32, -0.6343932747840881f32, -0.9951847195625305f32, -0.4713967442512512f32, 0.4713967442512512f32, 0.9951847195625305f32, 0.6343932747840881f32, -0.290284663438797f32, -0.9569403529167175f32, -0.7730104327201843f32, 0.0980171412229538f32, 0.8819212913513184f32, 0.8819212913513184f32, 0.0980171412229538f32, -0.7730104327201843f32, -0.9569403529167175f32, -0.290284663438797f32, 0.6343932747840881f32, 0.9951847195625305f32, 0.4713967442512512f32, -0.4713967442512512f32, -0.9951847195625305f32, -0.6343932747840881f32, 0.290284663438797f32, 0.9569403529167175f32, 0.7730104327201843f32, -0.0980171412229538f32, -0.8819212913513184f32],
    [-0.903989315032959f32, -0.24298018217086792f32, 0.5956993103027344f32, 0.9987954497337341f32, 0.6715589761734009f32, -0.1467304676771164f32, -0.8577286005020142f32, -0.9415440559387207f32, -0.3368898630142212f32, 0.5141027569770813f32, 0.9891765117645264f32, 0.7409511208534241f32, -0.049067676067352295f32, -0.803207516670227f32, -0.9700312614440918f32, -0.4275550842285156f32, 0.4275550842285156f32, 0.9700312614440918f32, 0.803207516670227f32, 0.049067676067352295f32, -0.7409511208534241f32, -0.9891765117645264f32, -0.5141027569770813f32, 0.3368898630142212f32, 0.9415440559387207f32, 0.8577286005020142f32, 0.1467304676771164f32, -0.6715589761734009f32, -0.9987954497337341f32, -0.5956993103027344f32, 0.24298018217086792f32, 0.903989315032959f32],
    [-0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32],
    [-0.9415440559387207f32, -0.5141027569770813f32, 0.1467304676771164f32, 0.7409511208534241f32, 0.9987954497337341f32, 0.803207516670227f32, 0.24298018217086792f32, -0.4275550842285156f32, -0.903989315032959f32, -0.9700312614440918f32, -0.5956993103027344f32, 0.049067676067352295f32, 0.6715589761734009f32, 0.9891765117645264f32, 0.8577286005020142f32, 0.3368898630142212f32, -0.3368898630142212f32, -0.8577286005020142f32, -0.9891765117645264f32, -0.6715589761734009f32, -0.049067676067352295f32, 0.5956993103027344f32, 0.9700312614440918f32, 0.903989315032959f32, 0.4275550842285156f32, -0.24298018217086792f32, -0.803207516670227f32, -0.9987954497337341f32, -0.7409511208534241f32, -0.1467304676771164f32, 0.5141027569770813f32, 0.9415440559387207f32],
    [-0.9569403529167175f32, -0.6343932747840881f32, -0.0980171412229538f32, 0.4713967442512512f32, 0.8819212913513184f32, 0.9951847195625305f32, 0.7730104327201843f32, 0.290284663438797f32, -0.290284663438797f32, -0.7730104327201843f32, -0.9951847195625305f32, -0.8819212913513184f32, -0.4713967442512512f32, 0.0980171412229538f32, 0.6343932747840881f32, 0.9569403529167175f32, 0.9569403529167175f32, 0.6343932747840881f32, 0.0980171412229538f32, -0.4713967442512512f32, -0.8819212913513184f32, -0.9951847195625305f32, -0.7730104327201843f32, -0.290284663438797f32, 0.290284663438797f32, 0.7730104327201843f32, 0.9951847195625305f32, 0.8819212913513184f32, 0.4713967442512512f32, -0.0980171412229538f32, -0.6343932747840881f32, -0.9569403529167175f32],
    [-0.9700312614440918f32, -0.7409511208534241f32, -0.3368898630142212f32, 0.1467304676771164f32, 0.5956993103027344f32, 0.903989315032959f32, 0.9987954497337341f32, 0.8577286005020142f32, 0.5141027569770813f32, 0.049067676067352295f32, -0.4275550842285156f32, -0.803207516670227f32, -0.9891765117645264f32, -0.9415440559387207f32, -0.6715589761734009f32, -0.24298018217086792f32, 0.24298018217086792f32, 0.6715589761734009f32, 0.9415440559387207f32, 0.9891765117645264f32, 0.803207516670227f32, 0.4275550842285156f32, -0.049067676067352295f32, -0.5141027569770813f32, -0.8577286005020142f32, -0.9987954497337341f32, -0.903989315032959f32, -0.5956993103027344f32, -0.1467304676771164f32, 0.3368898630142212f32, 0.7409511208534241f32, 0.9700312614440918f32],
    [-0.9807852506637573f32, -0.8314695954322815f32, -0.5555702447891235f32, -0.19509032368659973f32, 0.19509032368659973f32, 0.5555702447891235f32, 0.8314695954322815f32, 0.9807852506637573f32, 0.9807852506637573f32, 0.8314695954322815f32, 0.5555702447891235f32, 0.19509032368659973f32, -0.19509032368659973f32, -0.5555702447891235f32, -0.8314695954322815f32, -0.9807852506637573f32, -0.9807852506637573f32, -0.8314695954322815f32, -0.5555702447891235f32, -0.19509032368659973f32, 0.19509032368659973f32, 0.5555702447891235f32, 0.8314695954322815f32, 0.9807852506637573f32, 0.9807852506637573f32, 0.8314695954322815f32, 0.5555702447891235f32, 0.19509032368659973f32, -0.19509032368659973f32, -0.5555702447891235f32, -0.8314695954322815f32, -0.9807852506637573f32],
    [-0.9891765117645264f32, -0.903989315032959f32, -0.7409511208534241f32, -0.5141027569770813f32, -0.24298018217086792f32, 0.049067676067352295f32, 0.3368898630142212f32, 0.5956993103027344f32, 0.803207516670227f32, 0.9415440559387207f32, 0.9987954497337341f32, 0.9700312614440918f32, 0.8577286005020142f32, 0.6715589761734009f32, 0.4275550842285156f32, 0.1467304676771164f32, -0.1467304676771164f32, -0.4275550842285156f32, -0.6715589761734009f32, -0.8577286005020142f32, -0.9700312614440918f32, -0.9987954497337341f32, -0.9415440559387207f32, -0.803207516670227f32, -0.5956993103027344f32, -0.3368898630142212f32, -0.049067676067352295f32, 0.24298018217086792f32, 0.5141027569770813f32, 0.7409511208534241f32, 0.903989315032959f32, 0.9891765117645264f32],
    [-0.9951847195625305f32, -0.9569403529167175f32, -0.8819212913513184f32, -0.7730104327201843f32, -0.6343932747840881f32, -0.4713967442512512f32, -0.290284663438797f32, -0.0980171412229538f32, 0.0980171412229538f32, 0.290284663438797f32, 0.4713967442512512f32, 0.6343932747840881f32, 0.7730104327201843f32, 0.8819212913513184f32, 0.9569403529167175f32, 0.9951847195625305f32, 0.9951847195625305f32, 0.9569403529167175f32, 0.8819212913513184f32, 0.7730104327201843f32, 0.6343932747840881f32, 0.4713967442512512f32, 0.290284663438797f32, 0.0980171412229538f32, -0.0980171412229538f32, -0.290284663438797f32, -0.4713967442512512f32, -0.6343932747840881f32, -0.7730104327201843f32, -0.8819212913513184f32, -0.9569403529167175f32, -0.9951847195625305f32],
    [-0.9987954497337341f32, -0.9891765117645264f32, -0.9700312614440918f32, -0.9415440559387207f32, -0.903989315032959f32, -0.8577286005020142f32, -0.803207516670227f32, -0.7409511208534241f32, -0.6715589761734009f32, -0.5956993103027344f32, -0.5141027569770813f32, -0.4275550842285156f32, -0.3368898630142212f32, -0.24298018217086792f32, -0.1467304676771164f32, -0.049067676067352295f32, 0.049067676067352295f32, 0.1467304676771164f32, 0.24298018217086792f32, 0.3368898630142212f32, 0.4275550842285156f32, 0.5141027569770813f32, 0.5956993103027344f32, 0.6715589761734009f32, 0.7409511208534241f32, 0.803207516670227f32, 0.8577286005020142f32, 0.903989315032959f32, 0.9415440559387207f32, 0.9700312614440918f32, 0.9891765117645264f32, 0.9987954497337341f32],
    [-1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32, -1.0f32],
    [-0.9987954497337341f32, -0.9891765117645264f32, -0.9700312614440918f32, -0.9415440559387207f32, -0.903989315032959f32, -0.8577286005020142f32, -0.803207516670227f32, -0.7409511208534241f32, -0.6715589761734009f32, -0.5956993103027344f32, -0.5141027569770813f32, -0.4275550842285156f32, -0.3368898630142212f32, -0.24298018217086792f32, -0.1467304676771164f32, -0.049067676067352295f32, 0.049067676067352295f32, 0.1467304676771164f32, 0.24298018217086792f32, 0.3368898630142212f32, 0.4275550842285156f32, 0.5141027569770813f32, 0.5956993103027344f32, 0.6715589761734009f32, 0.7409511208534241f32, 0.803207516670227f32, 0.8577286005020142f32, 0.903989315032959f32, 0.9415440559387207f32, 0.9700312614440918f32, 0.9891765117645264f32, 0.9987954497337341f32],
    [-0.9951847195625305f32, -0.9569403529167175f32, -0.8819212913513184f32, -0.7730104327201843f32, -0.6343932747840881f32, -0.4713967442512512f32, -0.290284663438797f32, -0.0980171412229538f32, 0.0980171412229538f32, 0.290284663438797f32, 0.4713967442512512f32, 0.6343932747840881f32, 0.7730104327201843f32, 0.8819212913513184f32, 0.9569403529167175f32, 0.9951847195625305f32, 0.9951847195625305f32, 0.9569403529167175f32, 0.8819212913513184f32, 0.7730104327201843f32, 0.6343932747840881f32, 0.4713967442512512f32, 0.290284663438797f32, 0.0980171412229538f32, -0.0980171412229538f32, -0.290284663438797f32, -0.4713967442512512f32, -0.6343932747840881f32, -0.7730104327201843f32, -0.8819212913513184f32, -0.9569403529167175f32, -0.9951847195625305f32],
    [-0.9891765117645264f32, -0.903989315032959f32, -0.7409511208534241f32, -0.5141027569770813f32, -0.24298018217086792f32, 0.049067676067352295f32, 0.3368898630142212f32, 0.5956993103027344f32, 0.803207516670227f32, 0.9415440559387207f32, 0.9987954497337341f32, 0.9700312614440918f32, 0.8577286005020142f32, 0.6715589761734009f32, 0.4275550842285156f32, 0.1467304676771164f32, -0.1467304676771164f32, -0.4275550842285156f32, -0.6715589761734009f32, -0.8577286005020142f32, -0.9700312614440918f32, -0.9987954497337341f32, -0.9415440559387207f32, -0.803207516670227f32, -0.5956993103027344f32, -0.3368898630142212f32, -0.049067676067352295f32, 0.24298018217086792f32, 0.5141027569770813f32, 0.7409511208534241f32, 0.903989315032959f32, 0.9891765117645264f32],
    [-0.9807852506637573f32, -0.8314695954322815f32, -0.5555702447891235f32, -0.19509032368659973f32, 0.19509032368659973f32, 0.5555702447891235f32, 0.8314695954322815f32, 0.9807852506637573f32, 0.9807852506637573f32, 0.8314695954322815f32, 0.5555702447891235f32, 0.19509032368659973f32, -0.19509032368659973f32, -0.5555702447891235f32, -0.8314695954322815f32, -0.9807852506637573f32, -0.9807852506637573f32, -0.8314695954322815f32, -0.5555702447891235f32, -0.19509032368659973f32, 0.19509032368659973f32, 0.5555702447891235f32, 0.8314695954322815f32, 0.9807852506637573f32, 0.9807852506637573f32, 0.8314695954322815f32, 0.5555702447891235f32, 0.19509032368659973f32, -0.19509032368659973f32, -0.5555702447891235f32, -0.8314695954322815f32, -0.9807852506637573f32],
    [-0.9700312614440918f32, -0.7409511208534241f32, -0.3368898630142212f32, 0.1467304676771164f32, 0.5956993103027344f32, 0.903989315032959f32, 0.9987954497337341f32, 0.8577286005020142f32, 0.5141027569770813f32, 0.049067676067352295f32, -0.4275550842285156f32, -0.803207516670227f32, -0.9891765117645264f32, -0.9415440559387207f32, -0.6715589761734009f32, -0.24298018217086792f32, 0.24298018217086792f32, 0.6715589761734009f32, 0.9415440559387207f32, 0.9891765117645264f32, 0.803207516670227f32, 0.4275550842285156f32, -0.049067676067352295f32, -0.5141027569770813f32, -0.8577286005020142f32, -0.9987954497337341f32, -0.903989315032959f32, -0.5956993103027344f32, -0.1467304676771164f32, 0.3368898630142212f32, 0.7409511208534241f32, 0.9700312614440918f32],
    [-0.9569403529167175f32, -0.6343932747840881f32, -0.0980171412229538f32, 0.4713967442512512f32, 0.8819212913513184f32, 0.9951847195625305f32, 0.7730104327201843f32, 0.290284663438797f32, -0.290284663438797f32, -0.7730104327201843f32, -0.9951847195625305f32, -0.8819212913513184f32, -0.4713967442512512f32, 0.0980171412229538f32, 0.6343932747840881f32, 0.9569403529167175f32, 0.9569403529167175f32, 0.6343932747840881f32, 0.0980171412229538f32, -0.4713967442512512f32, -0.8819212913513184f32, -0.9951847195625305f32, -0.7730104327201843f32, -0.290284663438797f32, 0.290284663438797f32, 0.7730104327201843f32, 0.9951847195625305f32, 0.8819212913513184f32, 0.4713967442512512f32, -0.0980171412229538f32, -0.6343932747840881f32, -0.9569403529167175f32],
    [-0.9415440559387207f32, -0.5141027569770813f32, 0.1467304676771164f32, 0.7409511208534241f32, 0.9987954497337341f32, 0.803207516670227f32, 0.24298018217086792f32, -0.4275550842285156f32, -0.903989315032959f32, -0.9700312614440918f32, -0.5956993103027344f32, 0.049067676067352295f32, 0.6715589761734009f32, 0.9891765117645264f32, 0.8577286005020142f32, 0.3368898630142212f32, -0.3368898630142212f32, -0.8577286005020142f32, -0.9891765117645264f32, -0.6715589761734009f32, -0.049067676067352295f32, 0.5956993103027344f32, 0.9700312614440918f32, 0.903989315032959f32, 0.4275550842285156f32, -0.24298018217086792f32, -0.803207516670227f32, -0.9987954497337341f32, -0.7409511208534241f32, -0.1467304676771164f32, 0.5141027569770813f32, 0.9415440559387207f32],
    [-0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32],
    [-0.903989315032959f32, -0.24298018217086792f32, 0.5956993103027344f32, 0.9987954497337341f32, 0.6715589761734009f32, -0.1467304676771164f32, -0.8577286005020142f32, -0.9415440559387207f32, -0.3368898630142212f32, 0.5141027569770813f32, 0.9891765117645264f32, 0.7409511208534241f32, -0.049067676067352295f32, -0.803207516670227f32, -0.9700312614440918f32, -0.4275550842285156f32, 0.4275550842285156f32, 0.9700312614440918f32, 0.803207516670227f32, 0.049067676067352295f32, -0.7409511208534241f32, -0.9891765117645264f32, -0.5141027569770813f32, 0.3368898630142212f32, 0.9415440559387207f32, 0.8577286005020142f32, 0.1467304676771164f32, -0.6715589761734009f32, -0.9987954497337341f32, -0.5956993103027344f32, 0.24298018217086792f32, 0.903989315032959f32],
    [-0.8819212913513184f32, -0.0980171412229538f32, 0.7730104327201843f32, 0.9569403529167175f32, 0.290284663438797f32, -0.6343932747840881f32, -0.9951847195625305f32, -0.4713967442512512f32, 0.4713967442512512f32, 0.9951847195625305f32, 0.6343932747840881f32, -0.290284663438797f32, -0.9569403529167175f32, -0.7730104327201843f32, 0.0980171412229538f32, 0.8819212913513184f32, 0.8819212913513184f32, 0.0980171412229538f32, -0.7730104327201843f32, -0.9569403529167175f32, -0.290284663438797f32, 0.6343932747840881f32, 0.9951847195625305f32, 0.4713967442512512f32, -0.4713967442512512f32, -0.9951847195625305f32, -0.6343932747840881f32, 0.290284663438797f32, 0.9569403529167175f32, 0.7730104327201843f32, -0.0980171412229538f32, -0.8819212913513184f32],
    [-0.8577286005020142f32, 0.049067676067352295f32, 0.903989315032959f32, 0.803207516670227f32, -0.1467304676771164f32, -0.9415440559387207f32, -0.7409511208534241f32, 0.24298018217086792f32, 0.9700312614440918f32, 0.6715589761734009f32, -0.3368898630142212f32, -0.9891765117645264f32, -0.5956993103027344f32, 0.4275550842285156f32, 0.9987954497337341f32, 0.5141027569770813f32, -0.5141027569770813f32, -0.9987954497337341f32, -0.4275550842285156f32, 0.5956993103027344f32, 0.9891765117645264f32, 0.3368898630142212f32, -0.6715589761734009f32, -0.9700312614440918f32, -0.24298018217086792f32, 0.7409511208534241f32, 0.9415440559387207f32, 0.1467304676771164f32, -0.803207516670227f32, -0.903989315032959f32, -0.049067676067352295f32, 0.8577286005020142f32],
    [-0.8314695954322815f32, 0.19509032368659973f32, 0.9807852506637573f32, 0.5555702447891235f32, -0.5555702447891235f32, -0.9807852506637573f32, -0.19509032368659973f32, 0.8314695954322815f32, 0.8314695954322815f32, -0.19509032368659973f32, -0.9807852506637573f32, -0.5555702447891235f32, 0.5555702447891235f32, 0.9807852506637573f32, 0.19509032368659973f32, -0.8314695954322815f32, -0.8314695954322815f32, 0.19509032368659973f32, 0.9807852506637573f32, 0.5555702447891235f32, -0.5555702447891235f32, -0.9807852506637573f32, -0.19509032368659973f32, 0.8314695954322815f32, 0.8314695954322815f32, -0.19509032368659973f32, -0.9807852506637573f32, -0.5555702447891235f32, 0.5555702447891235f32, 0.9807852506637573f32, 0.19509032368659973f32, -0.8314695954322815f32],
    [-0.803207516670227f32, 0.3368898630142212f32, 0.9987954497337341f32, 0.24298018217086792f32, -0.8577286005020142f32, -0.7409511208534241f32, 0.4275550842285156f32, 0.9891765117645264f32, 0.1467304676771164f32, -0.903989315032959f32, -0.6715589761734009f32, 0.5141027569770813f32, 0.9700312614440918f32, 0.049067676067352295f32, -0.9415440559387207f32, -0.5956993103027344f32, 0.5956993103027344f32, 0.9415440559387207f32, -0.049067676067352295f32, -0.9700312614440918f32, -0.5141027569770813f32, 0.6715589761734009f32, 0.903989315032959f32, -0.1467304676771164f32, -0.9891765117645264f32, -0.4275550842285156f32, 0.7409511208534241f32, 0.8577286005020142f32, -0.24298018217086792f32, -0.9987954497337341f32, -0.3368898630142212f32, 0.803207516670227f32],
    [-0.7730104327201843f32, 0.4713967442512512f32, 0.9569403529167175f32, -0.0980171412229538f32, -0.9951847195625305f32, -0.290284663438797f32, 0.8819212913513184f32, 0.6343932747840881f32, -0.6343932747840881f32, -0.8819212913513184f32, 0.290284663438797f32, 0.9951847195625305f32, 0.0980171412229538f32, -0.9569403529167175f32, -0.4713967442512512f32, 0.7730104327201843f32, 0.7730104327201843f32, -0.4713967442512512f32, -0.9569403529167175f32, 0.0980171412229538f32, 0.9951847195625305f32, 0.290284663438797f32, -0.8819212913513184f32, -0.6343932747840881f32, 0.6343932747840881f32, 0.8819212913513184f32, -0.290284663438797f32, -0.9951847195625305f32, -0.0980171412229538f32, 0.9569403529167175f32, 0.4713967442512512f32, -0.7730104327201843f32],
    [-0.7409511208534241f32, 0.5956993103027344f32, 0.8577286005020142f32, -0.4275550842285156f32, -0.9415440559387207f32, 0.24298018217086792f32, 0.9891765117645264f32, -0.049067676067352295f32, -0.9987954497337341f32, -0.1467304676771164f32, 0.9700312614440918f32, 0.3368898630142212f32, -0.903989315032959f32, -0.5141027569770813f32, 0.803207516670227f32, 0.6715589761734009f32, -0.6715589761734009f32, -0.803207516670227f32, 0.5141027569770813f32, 0.903989315032959f32, -0.3368898630142212f32, -0.9700312614440918f32, 0.1467304676771164f32, 0.9987954497337341f32, 0.049067676067352295f32, -0.9891765117645264f32, -0.24298018217086792f32, 0.9415440559387207f32, 0.4275550842285156f32, -0.8577286005020142f32, -0.5956993103027344f32, 0.7409511208534241f32],
];

pub(crate) static IMDCT36_COS: [[f32; 36]; 18] = [
    [0.9990482330322266f32, 0.9914448857307434f32, 0.9762960076332092f32, 0.9537169337272644f32, 0.9238795042037964f32, 0.8870108127593994f32, 0.8433914184570312f32, 0.7933533191680908f32, 0.7372773289680481f32, 0.6755902171134949f32, 0.6087614297866821f32, 0.537299633026123f32, 0.4617486000061035f32, 0.3826834261417389f32, 0.30070579051971436f32, 0.2164396196603775f32, 0.13052618503570557f32, 0.04361938685178757f32, -0.04361938685178757f32, -0.13052618503570557f32, -0.2164396196603775f32, -0.30070579051971436f32, -0.3826834261417389f32, -0.4617486000061035f32, -0.537299633026123f32, -0.6087614297866821f32, -0.6755902171134949f32, -0.7372773289680481f32, -0.7933533191680908f32, -0.8433914184570312f32, -0.8870108127593994f32, -0.9238795042037964f32, -0.9537169337272644f32, -0.9762960076332092f32, -0.9914448857307434f32, -0.9990482330322266f32],
    [0.9914448857307434f32, 0.9238795042037964f32, 0.7933533191680908f32, 0.6087614297866821f32, 0.3826834261417389f32, 0.13052618503570557f32, -0.13052618503570557f32, -0.3826834261417389f32, -0.6087614297866821f32, -0.7933533191680908f32, -0.9238795042037964f32, -0.9914448857307434f32, -0.9914448857307434f32, -0.9238795042037964f32, -0.7933533191680908f32, -0.6087614297866821f32, -0.3826834261417389f32, -0.13052618503570557f32, 0.13052618503570557f32, 0.3826834261417389f32, 0.6087614297866821f32, 0.7933533191680908f32, 0.9238795042037964f32, 0.9914448857307434f32, 0.9914448857307434f32, 0.9238795042037964f32, 0.7933533191680908f32, 0.6087614297866821f32, 0.3826834261417389f32, 0.13052618503570557f32, -0.13052618503570557f32, -0.3826834261417389f32, -0.6087614297866821f32, -0.7933533191680908f32, -0.9238795042037964f32, -0.9914448857307434f32],
    [0.9762960076332092f32, 0.7933533191680908f32, 0.4617486000061035f32, 0.04361938685178757f32, -0.3826834261417389f32, -0.7372773289680481f32, -0.9537169337272644f32, -0.9914448857307434f32, -0.8433914184570312f32, -0.537299633026123f32, -0.13052618503570557f32, 0.30070579051971436f32, 0.6755902171134949f32, 0.9238795042037964f32, 0.9990482330322266f32, 0.8870108127593994f32, 0.6087614297866821f32, 0.2164396196603775f32, -0.2164396196603775f32, -0.6087614297866821f32, -0.8870108127593994f32, -0.9990482330322266f32, -0.9238795042037964f32, -0.6755902171134949f32, -0.30070579051971436f32, 0.13052618503570557f32, 0.537299633026123f32, 0.8433914184570312f32, 0.9914448857307434f32, 0.9537169337272644f32, 0.7372773289680481f32, 0.3826834261417389f32, -0.04361938685178757f32, -0.4617486000061035f32, -0.7933533191680908f32, -0.9762960076332092f32],
    [0.9537169337272644f32, 0.6087614297866821f32, 0.04361938685178757f32, -0.537299633026123f32, -0.9238795042037964f32, -0.9762960076332092f32, -0.6755902171134949f32, -0.13052618503570557f32, 0.4617486000061035f32, 0.8870108127593994f32, 0.9914448857307434f32, 0.7372773289680481f32, 0.2164396196603775f32, -0.3826834261417389f32, -0.8433914184570312f32, -0.9990482330322266f32, -0.7933533191680908f32, -0.30070579051971436f32, 0.30070579051971436f32, 0.7933533191680908f32, 0.9990482330322266f32, 0.8433914184570312f32, 0.3826834261417389f32, -0.2164396196603775f32, -0.7372773289680481f32, -0.9914448857307434f32, -0.8870108127593994f32, -0.4617486000061035f32, 0.13052618503570557f32, 0.6755902171134949f32, 0.9762960076332092f32, 0.9238795042037964f32, 0.537299633026123f32, -0.04361938685178757f32, -0.6087614297866821f32, -0.9537169337272644f32],
    [0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32],
    [0.8870108127593994f32, 0.13052618503570557f32, -0.7372773289680481f32, -0.9762960076332092f32, -0.3826834261417389f32, 0.537299633026123f32, 0.9990482330322266f32, 0.6087614297866821f32, -0.30070579051971436f32, -0.9537169337272644f32, -0.7933533191680908f32, 0.04361938685178757f32, 0.8433914184570312f32, 0.9238795042037964f32, 0.2164396196603775f32, -0.6755902171134949f32, -0.9914448857307434f32, -0.4617486000061035f32, 0.4617486000061035f32, 0.9914448857307434f32, 0.6755902171134949f32, -0.2164396196603775f32, -0.9238795042037964f32, -0.8433914184570312f32, -0.04361938685178757f32, 0.7933533191680908f32, 0.9537169337272644f32, 0.30070579051971436f32, -0.6087614297866821f32, -0.9990482330322266f32, -0.537299633026123f32, 0.3826834261417389f32, 0.9762960076332092f32, 0.7372773289680481f32, -0.13052618503570557f32, -0.8870108127593994f32],
    [0.8433914184570312f32, -0.13052618503570557f32, -0.9537169337272644f32, -0.6755902171134949f32, 0.3826834261417389f32, 0.9990482330322266f32, 0.4617486000061035f32, -0.6087614297866821f32, -0.9762960076332092f32, -0.2164396196603775f32, 0.7933533191680908f32, 0.8870108127593994f32, -0.04361938685178757f32, -0.9238795042037964f32, -0.7372773289680481f32, 0.30070579051971436f32, 0.9914448857307434f32, 0.537299633026123f32, -0.537299633026123f32, -0.9914448857307434f32, -0.30070579051971436f32, 0.7372773289680481f32, 0.9238795042037964f32, 0.04361938685178757f32, -0.8870108127593994f32, -0.7933533191680908f32, 0.2164396196603775f32, 0.9762960076332092f32, 0.6087614297866821f32, -0.4617486000061035f32, -0.9990482330322266f32, -0.3826834261417389f32, 0.6755902171134949f32, 0.9537169337272644f32, 0.13052618503570557f32, -0.8433914184570312f32],
    [0.7933533191680908f32, -0.3826834261417389f32, -0.9914448857307434f32, -0.13052618503570557f32, 0.9238795042037964f32, 0.6087614297866821f32, -0.6087614297866821f32, -0.9238795042037964f32, 0.13052618503570557f32, 0.9914448857307434f32, 0.3826834261417389f32, -0.7933533191680908f32, -0.7933533191680908f32, 0.3826834261417389f32, 0.9914448857307434f32, 0.13052618503570557f32, -0.9238795042037964f32, -0.6087614297866821f32, 0.6087614297866821f32, 0.9238795042037964f32, -0.13052618503570557f32, -0.9914448857307434f32, -0.3826834261417389f32, 0.7933533191680908f32, 0.7933533191680908f32, -0.3826834261417389f32, -0.9914448857307434f32, -0.13052618503570557f32, 0.9238795042037964f32, 0.6087614297866821f32, -0.6087614297866821f32, -0.9238795042037964f32, 0.13052618503570557f32, 0.9914448857307434f32, 0.3826834261417389f32, -0.7933533191680908f32],
    [0.7372773289680481f32, -0.6087614297866821f32, -0.8433914184570312f32, 0.4617486000061035f32, 0.9238795042037964f32, -0.30070579051971436f32, -0.9762960076332092f32, 0.13052618503570557f32, 0.9990482330322266f32, 0.04361938685178757f32, -0.9914448857307434f32, -0.2164396196603775f32, 0.9537169337272644f32, 0.3826834261417389f32, -0.8870108127593994f32, -0.537299633026123f32, 0.7933533191680908f32, 0.6755902171134949f32, -0.6755902171134949f32, -0.7933533191680908f32, 0.537299633026123f32, 0.8870108127593994f32, -0.3826834261417389f32, -0.9537169337272644f32, 0.2164396196603775f32, 0.9914448857307434f32, -0.04361938685178757f32, -0.9990482330322266f32, -0.13052618503570557f32, 0.9762960076332092f32, 0.30070579051971436f32, -0.9238795042037964f32, -0.4617486000061035f32, 0.8433914184570312f32, 0.6087614297866821f32, -0.7372773289680481f32],
    [0.6755902171134949f32, -0.7933533191680908f32, -0.537299633026123f32, 0.8870108127593994f32, 0.3826834261417389f32, -0.9537169337272644f32, -0.2164396196603775f32, 0.9914448857307434f32, 0.04361938685178757f32, -0.9990482330322266f32, 0.13052618503570557f32, 0.9762960076332092f32, -0.30070579051971436f32, -0.9238795042037964f32, 0.4617486000061035f32, 0.8433914184570312f32, -0.6087614297866821f32, -0.7372773289680481f32, 0.7372773289680481f32, 0.6087614297866821f32, -0.8433914184570312f32, -0.4617486000061035f32, 0.9238795042037964f32, 0.30070579051971436f32, -0.9762960076332092f32, -0.13052618503570557f32, 0.9990482330322266f32, -0.04361938685178757f32, -0.9914448857307434f32, 0.2164396196603775f32, 0.9537169337272644f32, -0.3826834261417389f32, -0.8870108127593994f32, 0.537299633026123f32, 0.7933533191680908f32, -0.6755902171134949f32],
    [0.6087614297866821f32, -0.9238795042037964f32, -0.13052618503570557f32, 0.9914448857307434f32, -0.3826834261417389f32, -0.7933533191680908f32, 0.7933533191680908f32, 0.3826834261417389f32, -0.9914448857307434f32, 0.13052618503570557f32, 0.9238795042037964f32, -0.6087614297866821f32, -0.6087614297866821f32, 0.9238795042037964f32, 0.13052618503570557f32, -0.9914448857307434f32, 0.3826834261417389f32, 0.7933533191680908f32, -0.7933533191680908f32, -0.3826834261417389f32, 0.9914448857307434f32, -0.13052618503570557f32, -0.9238795042037964f32, 0.6087614297866821f32, 0.6087614297866821f32, -0.9238795042037964f32, -0.13052618503570557f32, 0.9914448857307434f32, -0.3826834261417389f32, -0.7933533191680908f32, 0.7933533191680908f32, 0.3826834261417389f32, -0.9914448857307434f32, 0.13052618503570557f32, 0.9238795042037964f32, -0.6087614297866821f32],
    [0.537299633026123f32, -0.9914448857307434f32, 0.30070579051971436f32, 0.7372773289680481f32, -0.9238795042037964f32, 0.04361938685178757f32, 0.8870108127593994f32, -0.7933533191680908f32, -0.2164396196603775f32, 0.9762960076332092f32, -0.6087614297866821f32, -0.4617486000061035f32, 0.9990482330322266f32, -0.3826834261417389f32, -0.6755902171134949f32, 0.9537169337272644f32, -0.13052618503570557f32, -0.8433914184570312f32, 0.8433914184570312f32, 0.13052618503570557f32, -0.9537169337272644f32, 0.6755902171134949f32, 0.3826834261417389f32, -0.9990482330322266f32, 0.4617486000061035f32, 0.6087614297866821f32, -0.9762960076332092f32, 0.2164396196603775f32, 0.7933533191680908f32, -0.8870108127593994f32, -0.04361938685178757f32, 0.9238795042037964f32, -0.7372773289680481f32, -0.30070579051971436f32, 0.9914448857307434f32, -0.537299633026123f32],
    [0.4617486000061035f32, -0.9914448857307434f32, 0.6755902171134949f32, 0.2164396196603775f32, -0.9238795042037964f32, 0.8433914184570312f32, -0.04361938685178757f32, -0.7933533191680908f32, 0.9537169337272644f32, -0.30070579051971436f32, -0.6087614297866821f32, 0.9990482330322266f32, -0.537299633026123f32, -0.3826834261417389f32, 0.9762960076332092f32, -0.7372773289680481f32, -0.13052618503570557f32, 0.8870108127593994f32, -0.8870108127593994f32, 0.13052618503570557f32, 0.7372773289680481f32, -0.9762960076332092f32, 0.3826834261417389f32, 0.537299633026123f32, -0.9990482330322266f32, 0.6087614297866821f32, 0.30070579051971436f32, -0.9537169337272644f32, 0.7933533191680908f32, 0.04361938685178757f32, -0.8433914184570312f32, 0.9238795042037964f32, -0.2164396196603775f32, -0.6755902171134949f32, 0.9914448857307434f32, -0.4617486000061035f32],
    [0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32],
    [0.30070579051971436f32, -0.7933533191680908f32, 0.9990482330322266f32, -0.8433914184570312f32, 0.3826834261417389f32, 0.2164396196603775f32, -0.7372773289680481f32, 0.9914448857307434f32, -0.8870108127593994f32, 0.4617486000061035f32, 0.13052618503570557f32, -0.6755902171134949f32, 0.9762960076332092f32, -0.9238795042037964f32, 0.537299633026123f32, 0.04361938685178757f32, -0.6087614297866821f32, 0.9537169337272644f32, -0.9537169337272644f32, 0.6087614297866821f32, -0.04361938685178757f32, -0.537299633026123f32, 0.9238795042037964f32, -0.9762960076332092f32, 0.6755902171134949f32, -0.13052618503570557f32, -0.4617486000061035f32, 0.8870108127593994f32, -0.9914448857307434f32, 0.7372773289680481f32, -0.2164396196603775f32, -0.3826834261417389f32, 0.8433914184570312f32, -0.9990482330322266f32, 0.7933533191680908f32, -0.30070579051971436f32],
    [0.2164396196603775f32, -0.6087614297866821f32, 0.8870108127593994f32, -0.9990482330322266f32, 0.9238795042037964f32, -0.6755902171134949f32, 0.30070579051971436f32, 0.13052618503570557f32, -0.537299633026123f32, 0.8433914184570312f32, -0.9914448857307434f32, 0.9537169337272644f32, -0.7372773289680481f32, 0.3826834261417389f32, 0.04361938685178757f32, -0.4617486000061035f32, 0.7933533191680908f32, -0.9762960076332092f32, 0.9762960076332092f32, -0.7933533191680908f32, 0.4617486000061035f32, -0.04361938685178757f32, -0.3826834261417389f32, 0.7372773289680481f32, -0.9537169337272644f32, 0.9914448857307434f32, -0.8433914184570312f32, 0.537299633026123f32, -0.13052618503570557f32, -0.30070579051971436f32, 0.6755902171134949f32, -0.9238795042037964f32, 0.9990482330322266f32, -0.8870108127593994f32, 0.6087614297866821f32, -0.2164396196603775f32],
    [0.13052618503570557f32, -0.3826834261417389f32, 0.6087614297866821f32, -0.7933533191680908f32, 0.9238795042037964f32, -0.9914448857307434f32, 0.9914448857307434f32, -0.9238795042037964f32, 0.7933533191680908f32, -0.6087614297866821f32, 0.3826834261417389f32, -0.13052618503570557f32, -0.13052618503570557f32, 0.3826834261417389f32, -0.6087614297866821f32, 0.7933533191680908f32, -0.9238795042037964f32, 0.9914448857307434f32, -0.9914448857307434f32, 0.9238795042037964f32, -0.7933533191680908f32, 0.6087614297866821f32, -0.3826834261417389f32, 0.13052618503570557f32, 0.13052618503570557f32, -0.3826834261417389f32, 0.6087614297866821f32, -0.7933533191680908f32, 0.9238795042037964f32, -0.9914448857307434f32, 0.9914448857307434f32, -0.9238795042037964f32, 0.7933533191680908f32, -0.6087614297866821f32, 0.3826834261417389f32, -0.13052618503570557f32],
    [0.04361938685178757f32, -0.13052618503570557f32, 0.2164396196603775f32, -0.30070579051971436f32, 0.3826834261417389f32, -0.4617486000061035f32, 0.537299633026123f32, -0.6087614297866821f32, 0.6755902171134949f32, -0.7372773289680481f32, 0.7933533191680908f32, -0.8433914184570312f32, 0.8870108127593994f32, -0.9238795042037964f32, 0.9537169337272644f32, -0.9762960076332092f32, 0.9914448857307434f32, -0.9990482330322266f32, 0.9990482330322266f32, -0.9914448857307434f32, 0.9762960076332092f32, -0.9537169337272644f32, 0.9238795042037964f32, -0.8870108127593994f32, 0.8433914184570312f32, -0.7933533191680908f32, 0.7372773289680481f32, -0.6755902171134949f32, 0.6087614297866821f32, -0.537299633026123f32, 0.4617486000061035f32, -0.3826834261417389f32, 0.30070579051971436f32, -0.2164396196603775f32, 0.13052618503570557f32, -0.04361938685178757f32],
];

pub(crate) static IMDCT12_COS: [[f32; 12]; 6] = [
    [0.9914448857307434f32, 0.9238795042037964f32, 0.7933533191680908f32, 0.6087614297866821f32, 0.3826834261417389f32, 0.13052618503570557f32, -0.13052618503570557f32, -0.3826834261417389f32, -0.6087614297866821f32, -0.7933533191680908f32, -0.9238795042037964f32, -0.9914448857307434f32],
    [0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32, -0.9238795042037964f32, -0.3826834261417389f32, 0.3826834261417389f32, 0.9238795042037964f32, 0.9238795042037964f32, 0.3826834261417389f32, -0.3826834261417389f32, -0.9238795042037964f32],
    [0.7933533191680908f32, -0.3826834261417389f32, -0.9914448857307434f32, -0.13052618503570557f32, 0.9238795042037964f32, 0.6087614297866821f32, -0.6087614297866821f32, -0.9238795042037964f32, 0.13052618503570557f32, 0.9914448857307434f32, 0.3826834261417389f32, -0.7933533191680908f32],
    [0.6087614297866821f32, -0.9238795042037964f32, -0.13052618503570557f32, 0.9914448857307434f32, -0.3826834261417389f32, -0.7933533191680908f32, 0.7933533191680908f32, 0.3826834261417389f32, -0.9914448857307434f32, 0.13052618503570557f32, 0.9238795042037964f32, -0.6087614297866821f32],
    [0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32, -0.3826834261417389f32, 0.9238795042037964f32, -0.9238795042037964f32, 0.3826834261417389f32, 0.3826834261417389f32, -0.9238795042037964f32, 0.9238795042037964f32, -0.3826834261417389f32],
    [0.13052618503570557f32, -0.3826834261417389f32, 0.6087614297866821f32, -0.7933533191680908f32, 0.9238795042037964f32, -0.9914448857307434f32, 0.9914448857307434f32, -0.9238795042037964f32, 0.7933533191680908f32, -0.6087614297866821f32, 0.3826834261417389f32, -0.13052618503570557f32],
];

pub(crate) static IMDCT_WINDOW: [[f32; 36]; 4] = [
    [0.04361938685178757f32, 0.13052618503570557f32, 0.2164396196603775f32, 0.30070579051971436f32, 0.3826834261417389f32, 0.4617486000061035f32, 0.537299633026123f32, 0.6087614297866821f32, 0.6755902171134949f32, 0.7372773289680481f32, 0.7933533191680908f32, 0.8433914184570312f32, 0.8870108127593994f32, 0.9238795042037964f32, 0.9537169337272644f32, 0.9762960076332092f32, 0.9914448857307434f32, 0.9990482330322266f32, 0.9990482330322266f32, 0.9914448857307434f32, 0.9762960076332092f32, 0.9537169337272644f32, 0.9238795042037964f32, 0.8870108127593994f32, 0.8433914184570312f32, 0.7933533191680908f32, 0.7372773289680481f32, 0.6755902171134949f32, 0.6087614297866821f32, 0.537299633026123f32, 0.4617486000061035f32, 0.3826834261417389f32, 0.30070579051971436f32, 0.2164396196603775f32, 0.13052618503570557f32, 0.04361938685178757f32],
    [0.04361938685178757f32, 0.13052618503570557f32, 0.2164396196603775f32, 0.30070579051971436f32, 0.3826834261417389f32, 0.4617486000061035f32, 0.537299633026123f32, 0.6087614297866821f32, 0.6755902171134949f32, 0.7372773289680481f32, 0.7933533191680908f32, 0.8433914184570312f32, 0.8870108127593994f32, 0.9238795042037964f32, 0.9537169337272644f32, 0.9762960076332092f32, 0.9914448857307434f32, 0.9990482330322266f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 0.9914448857307434f32, 0.9238795042037964f32, 0.7933533191680908f32, 0.6087614297866821f32, 0.3826834261417389f32, 0.13052618503570557f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32],
    [0.13052618503570557f32, 0.3826834261417389f32, 0.6087614297866821f32, 0.7933533191680908f32, 0.9238795042037964f32, 0.9914448857307434f32, 0.9914448857307434f32, 0.9238795042037964f32, 0.7933533191680908f32, 0.6087614297866821f32, 0.3826834261417389f32, 0.13052618503570557f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32],
    [0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.13052618503570557f32, 0.3826834261417389f32, 0.6087614297866821f32, 0.7933533191680908f32, 0.9238795042037964f32, 0.9914448857307434f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 1.0f32, 0.9990482330322266f32, 0.9914448857307434f32, 0.9762960076332092f32, 0.9537169337272644f32, 0.9238795042037964f32, 0.8870108127593994f32, 0.8433914184570312f32, 0.7933533191680908f32, 0.7372773289680481f32, 0.6755902171134949f32, 0.6087614297866821f32, 0.537299633026123f32, 0.4617486000061035f32, 0.3826834261417389f32, 0.30070579051971436f32, 0.2164396196603775f32, 0.13052618503570557f32, 0.04361938685178757f32],
];

pub(crate) static ALIAS_CS: [f32; 8] = [
    0.8574929237365723f32, 0.881742000579834f32, 0.9496286511421204f32, 0.983314573764801f32, 0.9955177903175354f32, 0.9991605877876282f32, 0.9998992085456848f32, 0.9999931454658508f32,
];

pub(crate) static ALIAS_CA: [f32; 8] = [
    -0.5144957304000854f32, -0.471731960773468f32, -0.3133774399757385f32, -0.18191319704055786f32, -0.09457419067621231f32, -0.04096558317542076f32, -0.014198568649590015f32, -0.003699974622577429f32,
];

pub(crate) static PRETAB: [u8; 21] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2,
];

pub(crate) struct BandTable {
    pub long: [u16; 23],
    pub short: [u16; 14],
}

pub(crate) static BANDS: [BandTable; 3] = [
    BandTable {
        long: [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 52, 62, 74, 90, 110, 134, 162, 196, 238, 288, 342, 418, 576],
        short: [0, 4, 8, 12, 16, 22, 30, 40, 52, 66, 84, 106, 136, 192],
    },
    BandTable {
        long: [0, 4, 8, 12, 16, 20, 24, 30, 36, 42, 50, 60, 72, 88, 106, 128, 156, 190, 230, 276, 330, 384, 576],
        short: [0, 4, 8, 12, 16, 22, 28, 38, 50, 64, 80, 100, 126, 192],
    },
    BandTable {
        long: [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 54, 66, 82, 102, 126, 156, 194, 240, 296, 364, 448, 550, 576],
        short: [0, 4, 8, 12, 16, 22, 30, 42, 58, 78, 104, 138, 180, 192],
    },
];

