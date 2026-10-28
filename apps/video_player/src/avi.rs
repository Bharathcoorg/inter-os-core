//! Bounded RIFF/AVI reader for Motion JPEG video.
//!
//! WHAT this reads: the `hdrl` header of a RIFF `AVI ` file, then the `movi`
//! chunk stream in order. It understands `avih`, `strh`, `strf`, nested
//! `LIST` chunks (depth-limited) and `00dc`/`00db` video chunks, including the
//! one pad byte a chunk with an odd size carries. `idx1` is ignored: a file
//! that is read once, front to back, does not need an index.
//!
//! WHY it is written as a pull reader and not as a whole-file parser: the
//! platform's `FsRead` accepts at most 64 KiB per call and the filesystem
//! offers no seek, so the container is consumed strictly forwards through a
//! small buffer. A frame is the only part copied out, and its size is bounded
//! by the caller's buffer.
//!
//! Only Motion JPEG video is accepted. Other codecs are refused with
//! `AviError::UnsupportedCodec` rather than being presented as decodable.

use core::cmp;

/// The largest video frame payload the reader will hand to a caller.
pub const MAX_FRAME_BYTES: usize = 512 * 1024;

/// Bytes held by the reader between calls into the source.
const READ_BUFFER: usize = 4096;

/// How deep nested `LIST` chunks are followed inside `movi`.
const MAX_LIST_DEPTH: usize = 4;

/// A list whose stored size is unusable runs until the stream ends.
const UNKNOWN_LIMIT: u64 = u64::MAX;

/// Why an AVI file could not be opened or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AviError {
    /// The file does not start with `RIFF`.
    NotRiff,
    /// The RIFF form type is not `AVI `.
    NotAvi,
    /// The stream ends before a field it announced.
    Truncated,
    /// A chunk claims bytes its parent cannot contain.
    BadChunkSize,
    /// No `movi` list was found before the index or the end of the file.
    NoMovi,
    /// `hdrl` carries no `vids` stream.
    NoVideoStream,
    /// The video stream uses a codec this reader cannot decode.
    UnsupportedCodec([u8; 4]),
    /// Neither `avih` nor `strh` state a frame interval.
    NoFrameTiming,
    /// The frame is larger than the caller's buffer; it has been skipped.
    FrameTooLarge,
    /// The source refused a read.
    Io,
}

impl AviError {
    /// The sentence the player shows for this refusal.
    pub const fn message(self) -> &'static str {
        match self {
            AviError::NotRiff | AviError::NotAvi => "AVI: not a RIFF AVI file.",
            AviError::Truncated => "AVI: file ends inside its header or a chunk.",
            AviError::BadChunkSize => "AVI: chunk sizes do not fit their parent.",
            AviError::NoMovi => "AVI: no movi chunk before the index.",
            AviError::NoVideoStream => "AVI: no video stream in the header.",
            AviError::UnsupportedCodec(_) => {
                "AVI: video codec unsupported. Format unavailable."
            }
            AviError::NoFrameTiming => "AVI: header states no frame timing.",
            AviError::FrameTooLarge => "AVI: frame exceeds the player's buffer.",
            AviError::Io => "AVI: storage read failed.",
        }
    }
}

/// Where the reader pulls bytes from. A `SliceSource` serves host tests; the
/// application implements this over the capability-gated file descriptor.
pub trait Source {
    /// Reads at most `out.len()` bytes, returning 0 only at end of stream.
    fn read(&mut self, out: &mut [u8]) -> Result<usize, AviError>;
}

/// A `Source` over an in-memory slice.
pub struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceSource<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl Source for SliceSource<'_> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, AviError> {
        let n = cmp::min(out.len(), self.data.len() - self.pos);
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// What the header says about the movie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AviInfo {
    /// `avih.dwMicroSecPerFrame`, or `0` when the header left it empty.
    pub micros_per_frame: u32,
    /// `avih.dwTotalFrames`, or `0` when the header left it empty.
    pub total_frames: u32,
    /// Frame width from `strf`/`avih`.
    pub width: u32,
    /// Frame height from `strf`/`avih`.
    pub height: u32,
    /// Zero-based index of the first `vids` stream in `hdrl`.
    pub video_stream: Option<u8>,
    /// Number of `auds` streams in `hdrl`.
    pub audio_streams: u8,
    /// Codec fourcc: the `strf` compression, or the `strh` handler if unset.
    pub codec: [u8; 4],
    /// Interval derived from the video `strh` when `avih` omits one.
    strh_interval_us: u32,
    /// Zero-based index of the first `auds` stream in `hdrl`.
    pub audio_stream: Option<u8>,
    /// Audio format tag from `strf` (1 = WAVE_FORMAT_PCM).
    pub audio_format_tag: u16,
    /// Number of audio channels (e.g. 1 for mono, 2 for stereo).
    pub audio_channels: u16,
    /// Audio sample rate in Hz (e.g. 2000, 8000, 44100, 48000).
    pub audio_sample_rate: u32,
    /// Bits per audio sample (e.g. 8, 16).
    pub audio_bits_per_sample: u16,
}

impl AviInfo {
    fn empty() -> Self {
        Self {
            micros_per_frame: 0,
            total_frames: 0,
            width: 0,
            height: 0,
            video_stream: None,
            audio_streams: 0,
            codec: [0; 4],
            strh_interval_us: 0,
            audio_stream: None,
            audio_format_tag: 0,
            audio_channels: 0,
            audio_sample_rate: 0,
            audio_bits_per_sample: 0,
        }
    }

    /// True when the movie carries an uncompressed PCM audio stream.
    pub fn is_pcm_audio(&self) -> bool {
        self.audio_streams > 0 && self.audio_format_tag == 1
    }

    /// The frame interval the header states, in microseconds; `0` if unknown.
    pub fn frame_interval_us(&self) -> u32 {
        if self.micros_per_frame > 0 {
            self.micros_per_frame
        } else {
            self.strh_interval_us
        }
    }

    /// The frame interval in whole milliseconds, rounded up so playback can
    /// only run slower than the header asked, never faster.
    pub fn frame_interval_millis(&self) -> u32 {
        let us = self.frame_interval_us();
        if us == 0 {
            0
        } else {
            ((us as u64 + 999) / 1000) as u32
        }
    }

    /// Running time stated by the header, in whole seconds.
    pub fn duration_secs(&self) -> u32 {
        let interval = self.frame_interval_us() as u64;
        if interval == 0 || self.total_frames == 0 {
            return 0;
        }
        let secs = self.total_frames as u64 * interval / 1_000_000;
        cmp::min(secs, u32::MAX as u64) as u32
    }
}

/// Scales a frame interval for a playback speed given in thousandths.
///
/// At 10 fps (`100_000` us) and 1.0x this is 100 ms; at 2.0x it is 50 ms.
pub fn scale_interval_millis(interval_us: u32, speed_milli: u32) -> u32 {
    if interval_us == 0 || speed_milli == 0 {
        return 0;
    }
    let scaled_us = interval_us as u64 * 1000 / speed_milli as u64;
    ((scaled_us + 999) / 1000) as u32
}

/// The chunk id a video frame of `stream` carries: `00dc`, `01dc`, ...
fn frame_chunk_id(stream: u8) -> [u8; 4] {
    [b'0' + stream / 10, b'0' + stream % 10, b'd', b'c']
}

fn is_frame_chunk(id: &[u8; 4], stream: u8) -> bool {
    let expected = frame_chunk_id(stream);
    id[0] == expected[0]
        && id[1] == expected[1]
        && id[2] == b'd'
        && (id[3] == b'c' || id[3] == b'b')
}

/// The chunk id an audio chunk of `stream` carries: `00wb`, `01wb`, ...
fn audio_chunk_id(stream: u8) -> [u8; 4] {
    [b'0' + stream / 10, b'0' + stream % 10, b'w', b'b']
}

fn is_audio_chunk(id: &[u8; 4], stream: u8) -> bool {
    let expected = audio_chunk_id(stream);
    id[0] == expected[0]
        && id[1] == expected[1]
        && id[2] == b'w'
        && id[3] == b'b'
}

fn upper(b: u8) -> u8 {
    if b.is_ascii_lowercase() {
        b - 32
    } else {
        b
    }
}

fn fourcc_is(found: &[u8; 4], wanted: &[u8; 4]) -> bool {
    (0..4).all(|i| upper(found[i]) == wanted[i])
}

fn supported_codec(codec: &[u8; 4]) -> bool {
    fourcc_is(codec, b"MJPG") || fourcc_is(codec, b"JPEG")
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    (bytes[at] as u32)
        | ((bytes[at + 1] as u32) << 8)
        | ((bytes[at + 2] as u32) << 16)
        | ((bytes[at + 3] as u32) << 24)
}

/// A forward-only byte reader with a small buffer.
struct Reader<S: Source> {
    src: S,
    buf: [u8; READ_BUFFER],
    head: usize,
    tail: usize,
    /// Bytes consumed so far, used to bound nested chunk sizes.
    pos: u64,
}

impl<S: Source> Reader<S> {
    fn new(src: S) -> Self {
        Self {
            src,
            buf: [0; READ_BUFFER],
            head: 0,
            tail: 0,
            pos: 0,
        }
    }

    fn read_exact(&mut self, out: &mut [u8]) -> Result<(), AviError> {
        let mut written = 0;
        while written < out.len() {
            if self.head < self.tail {
                let n = cmp::min(self.tail - self.head, out.len() - written);
                out[written..written + n].copy_from_slice(&self.buf[self.head..self.head + n]);
                self.head += n;
                written += n;
                self.pos += n as u64;
                continue;
            }
            if out.len() - written >= READ_BUFFER {
                // A large request goes straight to the source; the source
                // returns at most what it was asked for, so nothing is lost.
                let n = self.src.read(&mut out[written..])?;
                if n == 0 {
                    return Err(AviError::Truncated);
                }
                written += n;
                self.pos += n as u64;
            } else {
                let n = self.src.read(&mut self.buf)?;
                if n == 0 {
                    return Err(AviError::Truncated);
                }
                self.head = 0;
                self.tail = n;
            }
        }
        Ok(())
    }

    fn skip(&mut self, mut n: u64) -> Result<(), AviError> {
        let buffered = (self.tail - self.head) as u64;
        let take = cmp::min(buffered, n);
        self.head += take as usize;
        self.pos += take;
        n -= take;
        let mut scratch = [0u8; READ_BUFFER];
        while n > 0 {
            let want = cmp::min(n as usize, scratch.len());
            self.read_exact(&mut scratch[..want])?;
            n -= want as u64;
        }
        Ok(())
    }

    fn u32le(&mut self) -> Result<u32, AviError> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(le32(&b, 0))
    }

    fn fourcc(&mut self) -> Result<[u8; 4], AviError> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(b)
    }
}

fn parse_avih<S: Source>(
    reader: &mut Reader<S>,
    size: u64,
    info: &mut AviInfo,
) -> Result<(), AviError> {
    if size < 40 {
        return Err(AviError::BadChunkSize);
    }
    let mut buf = [0u8; 40];
    reader.read_exact(&mut buf)?;
    if size > 40 {
        reader.skip(size - 40)?;
    }
    info.micros_per_frame = le32(&buf, 0);
    info.total_frames = le32(&buf, 16);
    info.width = le32(&buf, 32);
    info.height = le32(&buf, 36);
    Ok(())
}

fn parse_strl<S: Source>(
    reader: &mut Reader<S>,
    len: u64,
    info: &mut AviInfo,
    stream_count: &mut u32,
) -> Result<(), AviError> {
    let start = reader.pos;
    let end = start.checked_add(len).ok_or(AviError::BadChunkSize)?;
    let index = *stream_count;
    *stream_count = stream_count.saturating_add(1);

    let mut fcc_type = [0u8; 4];
    let mut handler = [0u8; 4];
    let mut scale = 0u32;
    let mut rate = 0u32;
    let mut strf_w = 0u32;
    let mut strf_h = 0u32;
    let mut strf_comp = [0u8; 4];
    let mut audio_tag = 0u16;
    let mut audio_channels = 0u16;
    let mut audio_rate = 0u32;
    let mut audio_bits = 0u16;

    while reader.pos + 8 <= end {
        let id = reader.fourcc()?;
        let size = reader.u32le()? as u64;
        let payload_end = reader.pos.checked_add(size).ok_or(AviError::BadChunkSize)?;
        if payload_end > end {
            return Err(AviError::BadChunkSize);
        }
        if &id == b"strh" {
            if size < 8 {
                return Err(AviError::BadChunkSize);
            }
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            fcc_type.copy_from_slice(&buf[0..4]);
            handler.copy_from_slice(&buf[4..8]);
            if size >= 28 {
                // dwScale and dwRate follow at offsets 20 and 24.
                reader.skip(12)?;
                let mut timing = [0u8; 8];
                reader.read_exact(&mut timing)?;
                scale = le32(&timing, 0);
                rate = le32(&timing, 4);
            }
        } else if &id == b"strf" {
            if size >= 14 {
                let mut buf = [0u8; 40];
                let take = cmp::min(size as usize, buf.len());
                reader.read_exact(&mut buf[..take])?;
                if take >= 20 {
                    strf_w = le32(&buf, 4);
                    let height = le32(&buf, 8) as i32;
                    strf_h = height.unsigned_abs();
                    strf_comp.copy_from_slice(&buf[16..20]);
                }
                if take >= 14 {
                    audio_tag = (buf[0] as u16) | ((buf[1] as u16) << 8);
                    audio_channels = (buf[2] as u16) | ((buf[3] as u16) << 8);
                    audio_rate = le32(&buf, 4);
                }
                if take >= 16 {
                    audio_bits = (buf[14] as u16) | ((buf[15] as u16) << 8);
                }
            }
        }
        if reader.pos < payload_end {
            reader.skip(payload_end - reader.pos)?;
        } else if reader.pos > payload_end {
            return Err(AviError::BadChunkSize);
        }
        if size & 1 == 1 {
            if payload_end + 1 > end {
                return Err(AviError::BadChunkSize);
            }
            reader.skip(1)?;
        }
    }
    if reader.pos < end {
        reader.skip(end - reader.pos)?;
    }

    if &fcc_type == b"vids" {
        if info.video_stream.is_none() && index <= 99 {
            info.video_stream = Some(index as u8);
            if strf_w > 0 {
                info.width = strf_w;
            }
            if strf_h > 0 {
                info.height = strf_h;
            }
            info.codec = if strf_comp != [0, 0, 0, 0] {
                strf_comp
            } else {
                handler
            };
            if scale > 0 && rate > 0 {
                let us = scale as u64 * 1_000_000 / rate as u64;
                info.strh_interval_us = cmp::min(us, u32::MAX as u64) as u32;
            }
        }
    } else if &fcc_type == b"auds" {
        info.audio_streams = info.audio_streams.saturating_add(1);
        if info.audio_stream.is_none() && index <= 99 {
            info.audio_stream = Some(index as u8);
            let tag = if audio_tag != 0 {
                audio_tag
            } else {
                let h_tag = (handler[0] as u16) | ((handler[1] as u16) << 8);
                if h_tag == 1 { 1 } else { 0 }
            };
            info.audio_format_tag = tag;
            info.audio_channels = audio_channels;
            info.audio_sample_rate = if audio_rate > 0 {
                audio_rate
            } else if rate > 0 && scale > 0 {
                rate / scale
            } else {
                0
            };
            info.audio_bits_per_sample = audio_bits;
        }
    }
    Ok(())
}

fn parse_hdrl<S: Source>(
    reader: &mut Reader<S>,
    len: u64,
    info: &mut AviInfo,
    stream_count: &mut u32,
) -> Result<(), AviError> {
    let start = reader.pos;
    let end = start.checked_add(len).ok_or(AviError::BadChunkSize)?;
    while reader.pos + 8 <= end {
        let id = reader.fourcc()?;
        let size = reader.u32le()? as u64;
        let payload_end = reader.pos.checked_add(size).ok_or(AviError::BadChunkSize)?;
        if payload_end > end {
            return Err(AviError::BadChunkSize);
        }
        if &id == b"LIST" {
            if size < 4 {
                return Err(AviError::BadChunkSize);
            }
            let ty = reader.fourcc()?;
            let inner_len = size - 4;
            if &ty == b"strl" {
                parse_strl(reader, inner_len, info, stream_count)?;
            } else {
                reader.skip(inner_len)?;
            }
        } else if &id == b"avih" {
            parse_avih(reader, size, info)?;
        } else {
            reader.skip(size)?;
        }
        if reader.pos < payload_end {
            reader.skip(payload_end - reader.pos)?;
        } else if reader.pos > payload_end {
            return Err(AviError::BadChunkSize);
        }
        if size & 1 == 1 {
            if payload_end + 1 > end {
                return Err(AviError::BadChunkSize);
            }
            reader.skip(1)?;
        }
    }
    if reader.pos < end {
        reader.skip(end - reader.pos)?;
    }
    Ok(())
}

/// The kind of chunk yielded by [`AviStream::next_chunk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AviChunk {
    /// A video frame payload (JPEG / MJPEG).
    Video(usize),
    /// An uncompressed PCM audio chunk (`##wb`).
    Audio(usize),
}

/// An open AVI, positioned at the first chunk of its `movi` list.
pub struct AviStream<S: Source> {
    reader: Reader<S>,
    info: AviInfo,
    /// Unread bytes of each open `LIST`, innermost last.
    limits: [u64; MAX_LIST_DEPTH],
    depth: usize,
    frames_read: u32,
    ended: bool,
}

impl<S: Source> AviStream<S> {
    /// Reads the RIFF header and `hdrl`, then stops at `movi`.
    pub fn open(source: S) -> Result<Self, AviError> {
        let mut reader = Reader::new(source);
        let riff = reader.fourcc()?;
        if &riff != b"RIFF" {
            return Err(AviError::NotRiff);
        }
        let riff_size = reader.u32le()?;
        let form = reader.fourcc()?;
        if &form != b"AVI " {
            return Err(AviError::NotAvi);
        }
        if riff_size < 4 {
            return Err(AviError::BadChunkSize);
        }

        let mut info = AviInfo::empty();
        let mut stream_count = 0u32;
        let mut saw_hdrl = false;
        loop {
            let id = reader.fourcc()?;
            let size = reader.u32le()? as u64;
            if &id == b"LIST" {
                if size < 4 {
                    return Err(AviError::BadChunkSize);
                }
                let ty = reader.fourcc()?;
                let payload = size - 4;
                if &ty == b"hdrl" {
                    parse_hdrl(&mut reader, payload, &mut info, &mut stream_count)?;
                    saw_hdrl = true;
                } else if &ty == b"movi" {
                    if !saw_hdrl {
                        return Err(AviError::NoVideoStream);
                    }
                    let Some(video_stream) = info.video_stream else {
                        return Err(AviError::NoVideoStream);
                    };
                    if !supported_codec(&info.codec) {
                        return Err(AviError::UnsupportedCodec(info.codec));
                    }
                    if info.frame_interval_us() == 0 {
                        return Err(AviError::NoFrameTiming);
                    }
                    let _ = video_stream;
                    let mut limits = [0u64; MAX_LIST_DEPTH];
                    limits[0] = if payload == 0 || payload == 0xFFFF_FFFF {
                        UNKNOWN_LIMIT
                    } else {
                        payload
                    };
                    return Ok(Self {
                        reader,
                        info,
                        limits,
                        depth: 1,
                        frames_read: 0,
                        ended: false,
                    });
                } else {
                    reader.skip(payload)?;
                }
            } else if &id == b"idx1" {
                return Err(AviError::NoMovi);
            } else {
                reader.skip(size + (size & 1))?;
            }
            if size & 1 == 1 {
                reader.skip(1)?;
            }
        }
    }

    /// The parsed header.
    pub fn info(&self) -> AviInfo {
        self.info
    }

    /// How many video frames have been handed out.
    pub fn frames_read(&self) -> u32 {
        self.frames_read
    }

    /// True once `movi` has been fully consumed.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Demuxes and yields the next payload chunk (`Video` or `Audio`) into `out`.
    ///
    /// Returns `Ok(None)` at end of stream. A chunk larger than `out` is
    /// consumed and reported as `FrameTooLarge`, leaving the reader aligned on
    /// the next chunk so a caller can go on.
    pub fn next_chunk(&mut self, out: &mut [u8]) -> Result<Option<AviChunk>, AviError> {
        if self.ended {
            return Ok(None);
        }
        let Some(video_stream) = self.info.video_stream else {
            return Ok(None);
        };
        let audio_stream = self.info.audio_stream;

        loop {
            while self.depth > 0 && self.limits[self.depth - 1] == 0 {
                self.depth -= 1;
            }
            if self.depth == 0 {
                self.ended = true;
                return Ok(None);
            }

            let remaining = self.limits[self.depth - 1];
            if remaining != UNKNOWN_LIMIT && remaining < 8 {
                // Trailing padding inside the innermost list.
                self.reader.skip(remaining)?;
                self.consume(remaining)?;
                self.limits[self.depth - 1] = 0;
                continue;
            }

            let id = match self.reader.fourcc() {
                Ok(id) => id,
                // A stream cut exactly at a chunk boundary ends cleanly.
                Err(AviError::Truncated) => {
                    self.ended = true;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let size = self.reader.u32le()? as u64;
            self.consume(8)?;

            if &id == b"idx1" && self.depth == 1 {
                // End of a movi whose stored size was left open.
                self.ended = true;
                return Ok(None);
            }

            if &id == b"LIST" {
                if size < 4 {
                    return Err(AviError::BadChunkSize);
                }
                let _ty = self.reader.fourcc()?;
                self.consume(4)?;
                let inner = size - 4;
                if self.depth >= MAX_LIST_DEPTH {
                    return Err(AviError::BadChunkSize);
                }
                let parent = self.limits[self.depth - 1];
                let child = inner + (size & 1);
                if parent != UNKNOWN_LIMIT && child > parent {
                    return Err(AviError::BadChunkSize);
                }
                self.limits[self.depth] = child;
                self.depth += 1;
                continue;
            }

            let padded = size + (size & 1);
            if is_frame_chunk(&id, video_stream) {
                if size as usize > out.len() {
                    self.reader.skip(padded)?;
                    self.consume(padded)?;
                    return Err(AviError::FrameTooLarge);
                }
                self.reader.read_exact(&mut out[..size as usize])?;
                self.consume(size)?;
                if size & 1 == 1 {
                    self.reader.skip(1)?;
                    self.consume(1)?;
                }
                self.frames_read = self.frames_read.saturating_add(1);
                return Ok(Some(AviChunk::Video(size as usize)));
            }

            if let Some(astream) = audio_stream {
                if is_audio_chunk(&id, astream) {
                    if size as usize > out.len() {
                        self.reader.skip(padded)?;
                        self.consume(padded)?;
                        return Err(AviError::FrameTooLarge);
                    }
                    self.reader.read_exact(&mut out[..size as usize])?;
                    self.consume(size)?;
                    if size & 1 == 1 {
                        self.reader.skip(1)?;
                        self.consume(1)?;
                    }
                    return Ok(Some(AviChunk::Audio(size as usize)));
                }
            }

            // Not a video frame or audio chunk: consume it and keep scanning.
            self.reader.skip(padded)?;
            self.consume(padded)?;
        }
    }

    /// Copies the next video frame payload into `out`, skipping any audio or junk chunks.
    ///
    /// Returns `Ok(None)` at end of stream. A frame larger than `out` is
    /// consumed and reported as `FrameTooLarge`, leaving the reader aligned on
    /// the next chunk so a caller can go on.
    pub fn next_frame(&mut self, out: &mut [u8]) -> Result<Option<usize>, AviError> {
        loop {
            match self.next_chunk(out)? {
                Some(AviChunk::Video(len)) => return Ok(Some(len)),
                Some(AviChunk::Audio(_)) => continue,
                None => return Ok(None),
            }
        }
    }

    fn consume(&mut self, n: u64) -> Result<(), AviError> {
        for i in 0..self.depth {
            if self.limits[i] == UNKNOWN_LIMIT {
                continue;
            }
            self.limits[i] = self.limits[i]
                .checked_sub(n)
                .ok_or(AviError::BadChunkSize)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_std::vec::Vec;

    /// One frame per tenth of a second, 64x48, from ffmpeg:
    /// `ffmpeg -f lavfi -i testsrc=size=64x48:rate=10:duration=1 \
    ///   -c:v mjpeg -pix_fmt yuvj420p -an fixture.avi`
    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/mjpeg_10fps.avi");
    /// A real ffmpeg-produced AVI carrying one MJPEG stream and one PCM
    /// stream: 64x48, 5 fps, 10 frames, 2 s. It is the fixture the QEMU
    /// verification played, so the unit test exercises the same bytes.
    const AUDIO_FIXTURE: &[u8] =
        include_bytes!("../tests/fixtures/mjpeg_5fps_audio.avi");

    fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() & 1 == 1 {
            out.push(0);
        }
    }

    fn push_list(out: &mut Vec<u8>, ty: &[u8; 4], body: &[u8]) {
        let size = (body.len() + 4) as u32;
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(ty);
        out.extend_from_slice(body);
        if size & 1 == 1 {
            out.push(0);
        }
    }

    struct StreamSpec {
        kind: [u8; 4],
        handler: [u8; 4],
        compression: [u8; 4],
        width: u32,
        height: u32,
        scale: u32,
        rate: u32,
        frames: u32,
    }

    fn video_spec() -> StreamSpec {
        StreamSpec {
            kind: *b"vids",
            handler: *b"MJPG",
            compression: *b"MJPG",
            width: 64,
            height: 48,
            scale: 1,
            rate: 10,
            frames: 2,
        }
    }

    fn audio_spec() -> StreamSpec {
        StreamSpec {
            kind: *b"auds",
            handler: *b"\0\0\0\0",
            compression: *b"\0\0\0\0",
            width: 0,
            height: 0,
            scale: 1,
            rate: 8000,
            frames: 2,
        }
    }

    /// Builds an AVI whose `movi` body is supplied verbatim.
    fn build_avi(streams: &[StreamSpec], movi: &[u8], micros: u32) -> Vec<u8> {
        let total_frames = streams
            .iter()
            .find(|s| &s.kind == b"vids")
            .map(|s| s.frames)
            .unwrap_or(0);
        let width = streams.iter().find(|s| &s.kind == b"vids").map(|s| s.width).unwrap_or(0);
        let height = streams.iter().find(|s| &s.kind == b"vids").map(|s| s.height).unwrap_or(0);

        let mut avih = [0u8; 56];
        avih[0..4].copy_from_slice(&micros.to_le_bytes());
        avih[16..20].copy_from_slice(&total_frames.to_le_bytes());
        avih[24..28].copy_from_slice(&(streams.len() as u32).to_le_bytes());
        avih[32..36].copy_from_slice(&width.to_le_bytes());
        avih[36..40].copy_from_slice(&height.to_le_bytes());

        let mut hdrl = Vec::new();
        push_chunk(&mut hdrl, b"avih", &avih);
        for spec in streams {
            let mut strh = [0u8; 56];
            strh[0..4].copy_from_slice(&spec.kind);
            strh[4..8].copy_from_slice(&spec.handler);
            strh[20..24].copy_from_slice(&spec.scale.to_le_bytes());
            strh[24..28].copy_from_slice(&spec.rate.to_le_bytes());
            strh[32..36].copy_from_slice(&spec.frames.to_le_bytes());
            let mut strl = Vec::new();
            push_chunk(&mut strl, b"strh", &strh);
            let mut strf = [0u8; 40];
            if &spec.kind == b"vids" {
                strf[0..4].copy_from_slice(&40u32.to_le_bytes());
                strf[4..8].copy_from_slice(&spec.width.to_le_bytes());
                strf[8..12].copy_from_slice(&spec.height.to_le_bytes());
                strf[12..14].copy_from_slice(&1u16.to_le_bytes());
                strf[14..16].copy_from_slice(&24u16.to_le_bytes());
                strf[16..20].copy_from_slice(&spec.compression);
            } else {
                strf[0..2].copy_from_slice(&1u16.to_le_bytes());
                strf[2..4].copy_from_slice(&1u16.to_le_bytes());
                strf[4..8].copy_from_slice(&8000u32.to_le_bytes());
                strf[14..16].copy_from_slice(&8u16.to_le_bytes());
            }
            push_chunk(&mut strl, b"strf", &strf);
            push_list(&mut hdrl, b"strl", &strl);
        }

        let mut body = Vec::new();
        push_list(&mut body, b"hdrl", &hdrl);
        push_list(&mut body, b"movi", movi);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        out.extend_from_slice(b"AVI ");
        out.extend_from_slice(&body);
        out
    }

    fn read_all(data: &[u8], out_len: usize) -> (AviInfo, Vec<Vec<u8>>) {
        let mut stream = AviStream::open(SliceSource::new(data)).expect("open fixture");
        let info = stream.info();
        let mut frame = host_std::vec![0u8; out_len];
        let mut frames = Vec::new();
        while let Some(n) = stream.next_frame(&mut frame).expect("frame") {
            frames.push(frame[..n].to_vec());
        }
        (info, frames)
    }

    #[test]
    fn fixture_is_a_riff_avi() {
        assert_eq!(&FIXTURE[0..4], b"RIFF");
        assert_eq!(&FIXTURE[8..12], b"AVI ");
    }

    #[test]
    fn fixture_yields_ten_frames_starting_at_soi() {
        let (info, frames) = read_all(FIXTURE, MAX_FRAME_BYTES);
        assert_eq!(info.width, 64);
        assert_eq!(info.height, 48);
        assert_eq!(info.video_stream, Some(0));
        assert_eq!(info.audio_streams, 0);
        assert_eq!(info.micros_per_frame, 100_000);
        assert_eq!(info.total_frames, 10);
        assert_eq!(frames.len(), 10);
        for frame in &frames {
            assert_eq!(&frame[0..2], &[0xFF, 0xD8], "00dc payload must start with SOI");
            assert_eq!(&frame[frame.len() - 2..], &[0xFF, 0xD9], "payload must end at EOI");
        }
    }

    /// The audio-bearing fixture must be reported as having audio and must
    /// still yield every video frame, because the player states the audio is
    /// unavailable instead of refusing the file.
    #[test]
    fn an_audio_stream_is_reported_and_the_video_still_decodes() {
        let (info, frames) = read_all(AUDIO_FIXTURE, MAX_FRAME_BYTES);
        assert_eq!(info.width, 64);
        assert_eq!(info.height, 48);
        assert_eq!(info.video_stream, Some(0));
        assert_eq!(info.audio_streams, 1);
        assert_eq!(info.micros_per_frame, 200_000);
        assert_eq!(info.total_frames, 10);
        assert_eq!(info.duration_secs(), 2);
        assert_eq!(frames.len(), 10);
        let mut rgba = host_std::vec![0u8; 64 * 48 * 4];
        for frame in &frames {
            assert_eq!(&frame[0..2], &[0xFF, 0xD8]);
            assert_eq!(
                crate::std::image::decode_jpeg(frame, &mut rgba),
                Ok((64, 48, 64 * 48 * 4))
            );
        }
    }

    #[test]
    fn pcm_audio_chunks_are_demuxed_and_decoded() {
        let mut stream = AviStream::open(SliceSource::new(AUDIO_FIXTURE)).expect("open");
        let info = stream.info();
        assert!(info.is_pcm_audio(), "fixture must report PCM audio stream");
        assert_eq!(info.audio_stream, Some(1));
        assert_eq!(info.audio_format_tag, 1);
        assert_eq!(info.audio_channels, 1);
        assert_eq!(info.audio_sample_rate, 2000);
        assert_eq!(info.audio_bits_per_sample, 8);

        let mut buf = host_std::vec![0u8; MAX_FRAME_BYTES];
        let mut video_frames = 0;
        let mut audio_chunks = 0;
        let mut total_audio_bytes = 0;

        while let Some(chunk) = stream.next_chunk(&mut buf).expect("next_chunk") {
            match chunk {
                AviChunk::Video(len) => {
                    video_frames += 1;
                    assert_eq!(&buf[0..2], &[0xFF, 0xD8]);
                    assert!(len > 0);
                }
                AviChunk::Audio(len) => {
                    audio_chunks += 1;
                    total_audio_bytes += len;
                }
            }
        }

        assert_eq!(video_frames, 10);
        assert_eq!(audio_chunks, 6);
        assert_eq!(total_audio_bytes, 200);
    }

    #[test]
    fn odd_chunks_are_padded_and_do_not_shift_the_next_frame() {
        let mut movi = Vec::new();
        push_chunk(&mut movi, b"00dc", &[0xFF, 0xD8, 0x01]); // odd: 3 + 1 pad
        push_chunk(&mut movi, b"00dc", &[0xFF, 0xD8, 0xFF, 0xD9]);
        let file = build_avi(&[video_spec()], &movi, 100_000);

        let (_, frames) = read_all(&file, MAX_FRAME_BYTES);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], host_std::vec![0xFF, 0xD8, 0x01]);
        assert_eq!(frames[1], host_std::vec![0xFF, 0xD8, 0xFF, 0xD9]);
    }

    #[test]
    fn truncated_header_is_refused() {
        let stream = AviStream::open(SliceSource::new(&FIXTURE[..10]));
        assert!(matches!(stream, Err(AviError::Truncated)));

        let mut cut = FIXTURE[..40].to_vec();
        cut.truncate(40);
        assert!(matches!(
            AviStream::open(SliceSource::new(&cut[..10])),
            Err(AviError::Truncated) | Err(AviError::NotAvi) | Err(AviError::NotRiff)
        ));
    }

    #[test]
    fn chunk_that_overflows_its_parent_is_refused() {
        let mut hdrl = Vec::new();
        let mut avih = [0u8; 56];
        avih[0..4].copy_from_slice(&100_000u32.to_le_bytes());
        push_chunk(&mut hdrl, b"avih", &avih);
        // A strl that claims more bytes than hdrl can hold.
        hdrl.extend_from_slice(b"LIST");
        hdrl.extend_from_slice(&10_000u32.to_le_bytes());
        hdrl.extend_from_slice(b"strl");

        let mut body = Vec::new();
        push_list(&mut body, b"hdrl", &hdrl);
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
        file.extend_from_slice(b"AVI ");
        file.extend_from_slice(&body);

        assert!(matches!(
            AviStream::open(SliceSource::new(&file)),
            Err(AviError::BadChunkSize) | Err(AviError::Truncated)
        ));
    }

    #[test]
    fn non_video_streams_are_ignored_and_video_need_not_be_stream_zero() {
        let mut movi = Vec::new();
        push_chunk(&mut movi, b"00wb", &[1, 2, 3, 4]); // stream 0 is audio
        push_chunk(&mut movi, b"01dc", &[0xFF, 0xD8, 0xAA, 0xBB]);
        push_chunk(&mut movi, b"00wb", &[5, 6]);
        push_chunk(&mut movi, b"01dc", &[0xFF, 0xD8, 0xCC, 0xDD]);
        let file = build_avi(&[audio_spec(), video_spec()], &movi, 100_000);

        let (info, frames) = read_all(&file, MAX_FRAME_BYTES);
        assert_eq!(info.video_stream, Some(1));
        assert_eq!(info.audio_streams, 1);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], host_std::vec![0xFF, 0xD8, 0xAA, 0xBB]);
        assert_eq!(frames[1], host_std::vec![0xFF, 0xD8, 0xCC, 0xDD]);
    }

    #[test]
    fn frames_inside_a_nested_list_are_found() {
        let mut rec = Vec::new();
        push_chunk(&mut rec, b"00dc", &[0xFF, 0xD8, 0x11, 0x22]);
        let mut movi = Vec::new();
        push_list(&mut movi, b"rec ", &rec);
        push_chunk(&mut movi, b"00dc", &[0xFF, 0xD8, 0x33, 0x44]);
        let file = build_avi(&[video_spec()], &movi, 100_000);

        let (_, frames) = read_all(&file, MAX_FRAME_BYTES);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], host_std::vec![0xFF, 0xD8, 0x11, 0x22]);
        assert_eq!(frames[1], host_std::vec![0xFF, 0xD8, 0x33, 0x44]);
    }

    #[test]
    fn oversized_frame_is_skipped_without_losing_alignment() {
        let mut movi = Vec::new();
        push_chunk(&mut movi, b"00dc", &[0xAA; 10]);
        push_chunk(&mut movi, b"00dc", &[0xFF, 0xD8, 0x55, 0x66]);
        let file = build_avi(&[video_spec()], &movi, 100_000);

        let mut stream = AviStream::open(SliceSource::new(&file)).expect("open");
        let mut small = [0u8; 4];
        assert_eq!(stream.next_frame(&mut small), Err(AviError::FrameTooLarge));
        assert_eq!(stream.next_frame(&mut small), Ok(Some(4)));
        assert_eq!(&small, &[0xFF, 0xD8, 0x55, 0x66]);
    }

    #[test]
    fn unsupported_codec_is_refused_by_name() {
        let mut spec = video_spec();
        spec.handler = *b"H264";
        spec.compression = *b"H264";
        let file = build_avi(&[spec], &[], 100_000);
        assert_eq!(
            AviStream::open(SliceSource::new(&file)).err(),
            Some(AviError::UnsupportedCodec(*b"H264"))
        );
    }

    #[test]
    fn missing_frame_timing_is_refused() {
        let mut spec = video_spec();
        spec.scale = 0;
        spec.rate = 0;
        let file = build_avi(&[spec], &[], 0);
        assert_eq!(
            AviStream::open(SliceSource::new(&file)).err(),
            Some(AviError::NoFrameTiming)
        );
    }

    #[test]
    fn fixture_frames_decode_with_the_platform_jpeg_service() {
        let (_, frames) = read_all(FIXTURE, MAX_FRAME_BYTES);
        let mut rgba = host_std::vec![0u8; 64 * 48 * 4];
        for frame in &frames {
            assert_eq!(
                crate::std::image::decode_jpeg(frame, &mut rgba),
                Ok((64, 48, 64 * 48 * 4))
            );
        }
    }

    #[test]
    fn pacing_math_uses_the_header_interval() {
        let (info, _) = read_all(FIXTURE, MAX_FRAME_BYTES);
        assert_eq!(info.frame_interval_us(), 100_000);
        assert_eq!(info.frame_interval_millis(), 100);
        assert_eq!(info.duration_secs(), 1);

        assert_eq!(scale_interval_millis(100_000, 1000), 100);
        assert_eq!(scale_interval_millis(100_000, 2000), 50);
        assert_eq!(scale_interval_millis(100_000, 500), 200);
        assert_eq!(scale_interval_millis(0, 1000), 0);
    }

    #[test]
    fn a_header_without_avih_timing_falls_back_to_strh() {
        let mut spec = video_spec();
        spec.scale = 1;
        spec.rate = 10;
        let file = build_avi(&[spec], &[], 0);
        let stream = AviStream::open(SliceSource::new(&file)).expect("open");
        assert_eq!(stream.info().frame_interval_millis(), 100);
    }
}
