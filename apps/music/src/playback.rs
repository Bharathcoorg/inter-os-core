//! Real playback of imported audio (WAV, FLAC, and MP3) over the platform's AC'97 stream.
//!
//! WHAT this is: the player reads the file the track was imported from, turns
//! its samples into the one PCM format the controller accepts (48 kHz, stereo,
//! 16-bit signed PCM), and hands them to the kernel's bounded stream ring.
//! The position the screen shows is the number of frames the device took,
//! read back from the stream, not a clock.
//!
//! WHY the conversion happens here and not in the kernel: the controller is
//! programmed for 48 kHz stereo 16-bit and does not resample. A file at any
//! other rate would be noise if it were handed over unchanged, so the player
//! converts before it writes and refuses a format it cannot decode.

use interos_std as std;
use interos_std::flac::{FlacDecoder, FlacError, StreamInfo};
use interos_std::mp3::{Mp3Decoder, Mp3Error};
use interos_std::resample::Resampler;
use interos_std::wav::{parse_wav, PcmConverter, WavError, WavInfo};
use std::AudioStreamStatus;

use crate::{library, position_millis, TrackStore, SOURCE_MAX};

/// Source bytes read from the file in one step.
pub const READ_BYTES: usize = 2_048;

/// Converted bytes handed to the stream in one step.
pub const PCM_BYTES: usize = 49_152;

/// Maximum decoded audio samples one frame of FLAC or MP3 can produce.
/// FLAC max block size is 4096 frames (8192 stereo samples).
/// MP3 max frame size is 1152 frames (2304 stereo samples).
const DECODED_SAMPLES_CAP: usize = 8_192;

/// Where the player is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayerState {
    Stopped,
    Playing,
    Paused,
    Ended,
}

/// Supported audio container and codec formats.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioFormat {
    Wav,
    Flac,
    Mp3,
}

/// Sniffs audio format by content inspection.
pub fn sniff_format(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        Some(AudioFormat::Wav)
    } else if bytes.len() >= 4 && &bytes[0..4] == b"fLaC" {
        Some(AudioFormat::Flac)
    } else if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        Some(AudioFormat::Mp3)
    } else if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        Some(AudioFormat::Mp3)
    } else {
        None
    }
}

/// Why the player could not play a track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaybackError {
    None,
    /// The track's record holds no file and the storage holds none with its
    /// name.
    NoSource,
    /// The filesystem refused to open the file.
    OpenRefused,
    /// The filesystem refused to read, or ended inside the file.
    ReadRefused,
    /// The file is not a supported WAV container.
    NotWav,
    /// The samples are in a format this player cannot decode.
    UnsupportedFormat,
    /// The device has no output stream, or refused to open one.
    StreamUnavailable,
    /// The stream refused the samples it was given.
    WriteRefused,
}

impl PlaybackError {
    /// What the screen may say about the refusal.
    pub const fn reason(self) -> &'static str {
        match self {
            PlaybackError::None => "",
            PlaybackError::NoSource => "the track's file was not found in storage",
            PlaybackError::OpenRefused => "the device refused to open the track's file",
            PlaybackError::ReadRefused => "the device refused to read the track's file",
            PlaybackError::NotWav => "the file is not a WAV container",
            PlaybackError::UnsupportedFormat => "the file's samples are not supported",
            PlaybackError::StreamUnavailable => "the device has no audio output stream",
            PlaybackError::WriteRefused => "the audio stream refused the samples",
        }
    }
}

/// Maps a header error to what the screen may say about it.
pub const fn error_for_wav(error: WavError) -> PlaybackError {
    match error {
        WavError::UnsupportedFormat => PlaybackError::UnsupportedFormat,
        WavError::NotRiff | WavError::NotWave | WavError::Truncated => PlaybackError::NotWav,
    }
}

/// Whether the device has finished every frame it was given for a track.
///
/// WHY the device's counter and not the player's own count of what it wrote:
/// the ring may still hold samples the controller has not read, and a track
/// that stopped as soon as its last write was queued would cut off its own
/// final third of a second.
///
/// WHY `device_running` must be clear too: `frames_consumed` counts frames
/// copied into the controller's DMA memory, which happens when a batch is
/// *submitted*. The last batch is therefore already counted while it is still
/// playing. Closing the stream on that count stops the bus master mid-batch
/// and truncates the track by up to one batch (about 170 ms), so the device
/// must report itself idle before the track is called finished.
pub const fn stream_drained(
    frames_written: u64,
    frames_consumed: u64,
    device_running: bool,
) -> bool {
    frames_written > 0 && frames_consumed >= frames_written && !device_running
}

#[repr(align(4))]
struct PcmBuffer([u8; PCM_BYTES]);

/// The read buffer shared by the header read and the streaming loop.
static mut READ_SCRATCH: [u8; READ_BYTES] = [0; READ_BYTES];
static mut PCM_SCRATCH: PcmBuffer = PcmBuffer([0; PCM_BYTES]);
static mut DECODED_SCRATCH: [i16; DECODED_SAMPLES_CAP] = [0; DECODED_SAMPLES_CAP];

/// Static decoders so they never occupy userspace stack space.
static mut FLAC_DECODER: FlacDecoder = FlacDecoder::new();
static mut MP3_DECODER: Mp3Decoder = Mp3Decoder::new();

/// The syscall code for a full ring: the caller should retry later.
const PIPE_FULL: i64 = -28;

/// One open track and the stream it is playing over.
pub struct Player {
    pub state: PlayerState,
    pub error: PlaybackError,
    /// The store index of the track this player opened.
    pub track: usize,
    /// Sniffed container / codec format.
    pub format: Option<AudioFormat>,
    /// The header of the WAV file being played, once read.
    pub info: Option<WavInfo>,
    /// The stream description of the FLAC file being played, once read.
    pub flac_info: Option<StreamInfo>,
    /// The last status the device reported for this stream.
    pub status: Option<AudioStreamStatus>,
    /// Software volume applied to the samples the player hands over.
    pub volume: u8,
    converter: Option<PcmConverter>,
    resampler: Option<Resampler>,
    fd: Option<u32>,
    cap: u64,
    stream: bool,
    /// Sample bytes the WAV file still holds after the bytes already read.
    data_remaining: u64,
    /// Source bytes read but not yet converted, carried across steps.
    pending: [u8; READ_BYTES],
    pending_len: usize,
    /// Converted bytes waiting for room in the ring.
    out_len: usize,
    out_pos: usize,
    /// Decoded samples waiting for room in the resampler output.
    decoded_len: usize,
    decoded_pos: usize,
    /// Whether the file stream hit EOF.
    file_eof: bool,
    /// Whether the decoder reached end-of-stream.
    decoder_done: bool,
}

impl Player {
    pub const EMPTY: Player = Player {
        state: PlayerState::Stopped,
        error: PlaybackError::None,
        track: 0,
        format: None,
        info: None,
        flac_info: None,
        status: None,
        volume: 100,
        converter: None,
        resampler: None,
        fd: None,
        cap: 0,
        stream: false,
        data_remaining: 0,
        pending: [0; READ_BYTES],
        pending_len: 0,
        out_len: 0,
        out_pos: 0,
        decoded_len: 0,
        decoded_pos: 0,
        file_eof: false,
        decoder_done: false,
    };

    /// What the screen may say when the player refused the current track.
    pub fn error_reason(&self) -> &'static str {
        self.error.reason()
    }

    /// Whether this player has a file open for the given track.
    pub fn holds(&self, track: usize) -> bool {
        self.format.is_some() && self.track == track && self.state != PlayerState::Stopped
    }

    /// Points the player at the track's new position after a reorder.
    pub fn retrack(&mut self, track: usize) {
        self.track = track;
    }

    /// The position the device has taken from this stream, in milliseconds.
    pub fn position_ms(&self) -> Option<u64> {
        self.status.map(|s| position_millis(s.frames_consumed))
    }

    /// The length the file claims, in milliseconds.
    ///
    /// For WAV and FLAC, the duration is calculated from the header facts.
    /// For MP3 without ID3 TLEN, None is returned truthfully.
    pub fn duration_ms(&self) -> Option<u64> {
        match self.format {
            Some(AudioFormat::Wav) => self.info.map(|info| info.duration_ms()),
            Some(AudioFormat::Flac) => self.flac_info.and_then(|info| {
                if info.total_samples > 0 && info.sample_rate > 0 {
                    Some((info.total_samples as u128 * 1000 / info.sample_rate as u128) as u64)
                } else {
                    None
                }
            }),
            Some(AudioFormat::Mp3) => None,
            None => None,
        }
    }

    /// Sets the software volume applied before samples reach the stream.
    pub fn set_volume(&mut self, percent: u8) {
        self.volume = percent.min(100);
        if let Some(converter) = self.converter.as_mut() {
            converter.set_volume_percent(self.volume);
        }
    }

    /// Opens the file the track was imported from and starts the stream.
    ///
    /// Returns `true` only when the device accepted the stream and the first
    /// samples were handed to it.
    pub fn start(&mut self, store: &TrackStore, track: usize) -> bool {
        let volume = self.volume;
        self.stop();
        self.volume = volume;
        self.track = track;

        let mut path = [0u8; SOURCE_MAX];
        let Some(path_len) = library::resolve_source(store, track, &mut path) else {
            self.error = PlaybackError::NoSource;
            return false;
        };
        let Ok(path_str) = core::str::from_utf8(&path[..path_len]) else {
            self.error = PlaybackError::OpenRefused;
            return false;
        };
        let fd = match std::fs_open(path_str, std::FS_READ) {
            Ok(fd) => fd,
            Err(_) => {
                self.error = PlaybackError::OpenRefused;
                return false;
            }
        };
        self.fd = Some(fd);

        let header = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };
        let read = match std::fs_read(fd, header) {
            Ok(n) if n > 0 => n,
            _ => {
                self.stop();
                self.error = PlaybackError::ReadRefused;
                return false;
            }
        };

        let Some(format) = sniff_format(&header[..read]) else {
            self.stop();
            self.error = PlaybackError::UnsupportedFormat;
            return false;
        };
        self.format = Some(format);

        match format {
            AudioFormat::Wav => {
                let (info, data_offset) = match parse_wav(&header[..read]) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        self.stop();
                        self.error = error_for_wav(error);
                        return false;
                    }
                };
                self.info = Some(info);
                self.data_remaining = info.data_len;

                let mut consumed = read as u64;
                while consumed < data_offset as u64 {
                    let want = ((data_offset as u64) - consumed).min(READ_BYTES as u64) as usize;
                    match std::fs_read(fd, &mut header[..want]) {
                        Ok(0) => {
                            self.stop();
                            self.error = PlaybackError::ReadRefused;
                            return false;
                        }
                        Ok(n) => consumed += n as u64,
                        Err(_) => {
                            self.stop();
                            self.error = PlaybackError::ReadRefused;
                            return false;
                        }
                    }
                }
                if (data_offset as u64) < consumed {
                    let first = data_offset;
                    let available = (consumed - data_offset as u64).min(info.data_len) as usize;
                    let available = available.min(READ_BYTES);
                    self.pending[..available].copy_from_slice(&header[first..first + available]);
                    self.pending_len = available;
                    self.data_remaining -= available as u64;
                }
                if info.data_len == 0 {
                    self.finish();
                    return true;
                }

                let Ok(mut converter) = PcmConverter::new(info) else {
                    self.stop();
                    self.error = PlaybackError::UnsupportedFormat;
                    return false;
                };
                converter.set_volume_percent(self.volume);
                self.converter = Some(converter);
            }
            AudioFormat::Flac => {
                let flac = unsafe { &mut *core::ptr::addr_of_mut!(FLAC_DECODER) };
                *flac = FlacDecoder::new();
                let pushed = flac.push(&header[..read]);
                if pushed < read {
                    let leftover = read - pushed;
                    self.pending[..leftover].copy_from_slice(&header[pushed..read]);
                    self.pending_len = leftover;
                }
            }
            AudioFormat::Mp3 => {
                let mp3 = unsafe { &mut *core::ptr::addr_of_mut!(MP3_DECODER) };
                mp3.reset();
                let avail = mp3.available();
                let take = read.min(avail);
                let _ = mp3.push(&header[..take]);
                if take < read {
                    let leftover = read - take;
                    self.pending[..leftover].copy_from_slice(&header[take..read]);
                    self.pending_len = leftover;
                }
            }
        }

        let Ok(cap) = std::capability_handle(std::cap_type::AUDIO_STREAM, 0) else {
            self.stop();
            self.error = PlaybackError::StreamUnavailable;
            return false;
        };
        if std::audio_stream_open(cap, std::AUDIO_STREAM_FORMAT_NATIVE).is_err() {
            self.stop();
            self.error = PlaybackError::StreamUnavailable;
            return false;
        }
        self.cap = cap;
        self.stream = true;
        self.state = PlayerState::Playing;
        self.pump();
        true
    }

    /// Stops enqueueing samples. The device keeps what it already holds.
    pub fn pause(&mut self) {
        if self.state == PlayerState::Playing {
            self.state = PlayerState::Paused;
        }
    }

    /// Continues enqueueing samples from where the file was left.
    pub fn resume(&mut self) {
        if self.state == PlayerState::Paused && self.format.is_some() {
            self.state = PlayerState::Playing;
            self.pump();
        }
    }

    /// Seeks to a position in milliseconds within the current track.
    ///
    /// Reopens the track and skips decoded/uncompressed frames up to `target_ms`.
    /// Returns `true` if seek succeeded.
    pub fn seek(&mut self, store: &TrackStore, target_ms: u64) -> bool {
        if !self.holds(self.track) {
            return false;
        }
        let track = self.track;
        let volume = self.volume;
        let was_playing = self.state == PlayerState::Playing;

        if !self.start(store, track) {
            return false;
        }
        self.set_volume(volume);

        match self.format {
            Some(AudioFormat::Wav) => {
                let Some(info) = self.info else { return false; };
                let frame_size = (info.channels as u64) * (info.bits_per_sample as u64 / 8);
                if frame_size == 0 || info.sample_rate == 0 { return false; }
                let target_frames = (target_ms * info.sample_rate as u64) / 1000;
                let target_bytes = target_frames * frame_size;
                let skip_bytes = target_bytes.min(info.data_len);

                let from_pending = (self.pending_len as u64).min(skip_bytes);
                self.pending_len -= from_pending as usize;
                let mut remaining_to_skip = skip_bytes - from_pending;

                let fd = match self.fd {
                    Some(fd) => fd,
                    None => return false,
                };
                let scratch = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };
                while remaining_to_skip > 0 {
                    let want = (remaining_to_skip as usize).min(READ_BYTES);
                    match std::fs_read(fd, &mut scratch[..want]) {
                        Ok(0) => break,
                        Ok(n) => {
                            remaining_to_skip -= n as u64;
                            self.data_remaining = self.data_remaining.saturating_sub(n as u64);
                        }
                        Err(_) => return false,
                    }
                }
            }
            Some(AudioFormat::Flac) => {
                let Some(flac_info) = self.flac_info else { return false; };
                if flac_info.sample_rate == 0 { return false; }
                let target_samples = ((target_ms * flac_info.sample_rate as u64) / 1000) * flac_info.channels as u64;
                let mut skipped = 0u64;
                let flac = unsafe { &mut *core::ptr::addr_of_mut!(FLAC_DECODER) };
                let decoded_buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODED_SCRATCH) };
                let fd = match self.fd {
                    Some(fd) => fd,
                    None => return false,
                };
                let read_buf = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };

                while skipped < target_samples {
                    match flac.next_frame(decoded_buf) {
                        Ok(Some(info)) => {
                            skipped += info.samples as u64;
                        }
                        Ok(None) => {
                            match std::fs_read(fd, read_buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    flac.push(&read_buf[..n]);
                                }
                                Err(_) => break,
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            Some(AudioFormat::Mp3) => {
                let target_samples = ((target_ms * 44100) / 1000) * 2;
                let mut skipped = 0u64;
                let mp3 = unsafe { &mut *core::ptr::addr_of_mut!(MP3_DECODER) };
                let decoded_buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODED_SCRATCH) };
                let fd = match self.fd {
                    Some(fd) => fd,
                    None => return false,
                };
                let read_buf = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };

                while skipped < target_samples {
                    match mp3.next_frame(decoded_buf) {
                        Ok(Some(info)) => {
                            skipped += info.samples() as u64;
                        }
                        Ok(None) => {
                            match std::fs_read(fd, read_buf) {
                                Ok(0) => break,
                                Ok(n) => {
                                    let avail = mp3.available();
                                    let take = n.min(avail);
                                    let _ = mp3.push(&read_buf[..take]);
                                }
                                Err(_) => break,
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            None => return false,
        }

        if was_playing {
            self.state = PlayerState::Playing;
            self.pump();
        } else {
            self.state = PlayerState::Paused;
        }
        true
    }

    /// Closes the file and the stream and forgets the track.
    pub fn stop(&mut self) {
        if self.stream {
            let _ = std::audio_stream_close(self.cap);
        }
        if let Some(fd) = self.fd.take() {
            let _ = std::fs_close(fd);
        }
        let volume = self.volume;
        *self = Player::EMPTY;
        self.volume = volume;
    }

    /// Closes the file and the stream, keeping what was played.
    fn finish(&mut self) {
        if self.stream {
            let _ = std::audio_stream_close(self.cap);
        }
        if let Some(fd) = self.fd.take() {
            let _ = std::fs_close(fd);
        }
        self.stream = false;
        self.converter = None;
        self.resampler = None;
        self.state = PlayerState::Ended;
    }

    /// Polls the device and returns what it reported.
    pub fn poll(&mut self) -> Option<AudioStreamStatus> {
        if self.stream {
            match std::audio_stream_status(self.cap) {
                Ok(status) => self.status = Some(status),
                Err(code) => {
                    static REPORTED: core::sync::atomic::AtomicBool =
                        core::sync::atomic::AtomicBool::new(false);
                    if !REPORTED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                        std::log("[MUSIC APP] The device refused to report the stream's state.").ok();
                        let _ = code;
                    }
                }
            }
        }
        self.status
    }

    /// Polls, feeds the stream, and notices when the track has played out.
    pub fn tick(&mut self) {
        self.poll();
        if self.state != PlayerState::Playing || self.format.is_none() {
            return;
        }
        self.pump();
        self.poll();
        let track_finished = match self.format {
            Some(AudioFormat::Wav) => {
                self.data_remaining == 0 && self.pending_len == 0 && self.out_pos >= self.out_len
            }
            Some(AudioFormat::Flac | AudioFormat::Mp3) => {
                self.file_eof
                    && self.decoder_done
                    && self.decoded_pos >= self.decoded_len
                    && self.out_pos >= self.out_len
            }
            None => false,
        };
        if track_finished {
            if let Some(status) = self.status {
                if stream_drained(
                    status.frames_written,
                    status.frames_consumed,
                    status.device_running,
                ) {
                    self.finish();
                }
            }
        }
    }

    /// Hands the stream as many converted samples as it will take.
    ///
    /// Returns the number of bytes the stream accepted.
    pub fn pump(&mut self) -> usize {
        if self.state != PlayerState::Playing || !self.stream {
            return 0;
        }
        let Some(fd) = self.fd else {
            return 0;
        };
        let pcm = unsafe { &mut *core::ptr::addr_of_mut!(PCM_SCRATCH) };
        let mut accepted_total = 0usize;
        loop {
            if self.out_pos < self.out_len {
                match std::audio_stream_write(self.cap, &pcm.0[self.out_pos..self.out_len]) {
                    Ok(0) => break,
                    Ok(n) => {
                        accepted_total += n;
                        self.out_pos += n;
                        if self.out_pos < self.out_len {
                            break;
                        }
                        self.out_pos = 0;
                        self.out_len = 0;
                        continue;
                    }
                    Err(PIPE_FULL) => break,
                    Err(_) => {
                        self.error = PlaybackError::WriteRefused;
                        break;
                    }
                }
            }

            match self.format {
                Some(AudioFormat::Wav) => {
                    if self.pending_len == 0 {
                        if self.data_remaining == 0 {
                            break;
                        }
                        let want = (READ_BYTES as u64).min(self.data_remaining) as usize;
                        match std::fs_read(fd, &mut self.pending[..want]) {
                            Ok(0) => {
                                self.data_remaining = 0;
                                break;
                            }
                            Ok(n) => {
                                self.pending_len = n;
                                self.data_remaining = self.data_remaining.saturating_sub(n as u64);
                            }
                            Err(_) => {
                                self.error = PlaybackError::ReadRefused;
                                break;
                            }
                        }
                    }
                    let Some(converter) = self.converter.as_mut() else {
                        break;
                    };
                    let (consumed, written) = converter.convert(&self.pending[..self.pending_len], &mut pcm.0);
                    if consumed == 0 && written == 0 {
                        break;
                    }
                    if consumed < self.pending_len {
                        self.pending.copy_within(consumed..self.pending_len, 0);
                    }
                    self.pending_len -= consumed;
                    if written == 0 {
                        continue;
                    }
                    self.out_pos = 0;
                    self.out_len = written;
                }
                Some(AudioFormat::Flac) => {
                    let flac = unsafe { &mut *core::ptr::addr_of_mut!(FLAC_DECODER) };
                    let decoded_buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODED_SCRATCH) };
                    let out_pcm_16 = unsafe {
                        core::slice::from_raw_parts_mut(
                            pcm.0.as_mut_ptr() as *mut i16,
                            PCM_BYTES / 2,
                        )
                    };

                    if self.decoded_pos < self.decoded_len {
                        let Some(resampler) = self.resampler.as_mut() else { break; };
                        let in_samples = &decoded_buf[self.decoded_pos..self.decoded_len];
                        let (consumed_frames, written_frames) = resampler.process(in_samples, out_pcm_16);
                        let consumed_samples = consumed_frames * resampler.src_channels() as usize;
                        self.decoded_pos += consumed_samples;
                        if written_frames > 0 {
                            scale_pcm_volume(&mut out_pcm_16[..written_frames * 2], self.volume);
                            self.out_pos = 0;
                            self.out_len = written_frames * 4;
                            continue;
                        }
                    }
                    self.decoded_pos = 0;
                    self.decoded_len = 0;

                    if self.pending_len > 0 {
                        let pushed = flac.push(&self.pending[..self.pending_len]);
                        if pushed > 0 {
                            if pushed < self.pending_len {
                                self.pending.copy_within(pushed..self.pending_len, 0);
                            }
                            self.pending_len -= pushed;
                        }
                    }

                    match flac.next_frame(decoded_buf) {
                        Ok(Some(frame_info)) => {
                            if self.flac_info.is_none() {
                                self.flac_info = flac.stream_info();
                            }
                            if self.resampler.is_none() {
                                let rate = frame_info.sample_rate;
                                let channels = frame_info.channels;
                                match Resampler::new(rate, channels) {
                                    Ok(r) => self.resampler = Some(r),
                                    Err(_) => {
                                        self.error = PlaybackError::UnsupportedFormat;
                                        break;
                                    }
                                }
                            }
                            let Some(resampler) = self.resampler.as_mut() else { break; };
                            let (consumed_frames, written_frames) =
                                resampler.process(&decoded_buf[..frame_info.samples], out_pcm_16);
                            let consumed_samples = consumed_frames * resampler.src_channels() as usize;
                            if consumed_samples < frame_info.samples {
                                self.decoded_pos = consumed_samples;
                                self.decoded_len = frame_info.samples;
                            }
                            if written_frames > 0 {
                                scale_pcm_volume(&mut out_pcm_16[..written_frames * 2], self.volume);
                                self.out_pos = 0;
                                self.out_len = written_frames * 4;
                            }
                        }
                        Ok(None) => {
                            if self.file_eof {
                                if let Some(resampler) = self.resampler.as_mut() {
                                    let drained = resampler.drain(out_pcm_16);
                                    if drained > 0 {
                                        scale_pcm_volume(&mut out_pcm_16[..drained * 2], self.volume);
                                        self.out_pos = 0;
                                        self.out_len = drained * 4;
                                        continue;
                                    }
                                }
                                self.decoder_done = true;
                                break;
                            }
                            let read_buf = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };
                            match std::fs_read(fd, read_buf) {
                                Ok(0) => {
                                    self.file_eof = true;
                                    flac.finish();
                                }
                                Ok(n) => {
                                    let pushed = flac.push(&read_buf[..n]);
                                    if pushed < n {
                                        let leftover = n - pushed;
                                        let can_take = leftover.min(READ_BYTES - self.pending_len);
                                        self.pending[self.pending_len..self.pending_len + can_take]
                                            .copy_from_slice(&read_buf[pushed..pushed + can_take]);
                                        self.pending_len += can_take;
                                    }
                                }
                                Err(_) => {
                                    self.error = PlaybackError::ReadRefused;
                                    break;
                                }
                            }
                        }
                        Err(FlacError::Truncated) => {
                            self.file_eof = true;
                            self.decoder_done = true;
                            break;
                        }
                        Err(_) => {
                            self.error = PlaybackError::UnsupportedFormat;
                            break;
                        }
                    }
                }
                Some(AudioFormat::Mp3) => {
                    let mp3 = unsafe { &mut *core::ptr::addr_of_mut!(MP3_DECODER) };
                    let decoded_buf = unsafe { &mut *core::ptr::addr_of_mut!(DECODED_SCRATCH) };
                    let out_pcm_16 = unsafe {
                        core::slice::from_raw_parts_mut(
                            pcm.0.as_mut_ptr() as *mut i16,
                            PCM_BYTES / 2,
                        )
                    };

                    if self.decoded_pos < self.decoded_len {
                        let Some(resampler) = self.resampler.as_mut() else { break; };
                        let in_samples = &decoded_buf[self.decoded_pos..self.decoded_len];
                        let (consumed_frames, written_frames) = resampler.process(in_samples, out_pcm_16);
                        let consumed_samples = consumed_frames * resampler.src_channels() as usize;
                        self.decoded_pos += consumed_samples;
                        if written_frames > 0 {
                            scale_pcm_volume(&mut out_pcm_16[..written_frames * 2], self.volume);
                            self.out_pos = 0;
                            self.out_len = written_frames * 4;
                            continue;
                        }
                    }
                    self.decoded_pos = 0;
                    self.decoded_len = 0;

                    if self.pending_len > 0 {
                        let avail = mp3.available();
                        let take = self.pending_len.min(avail);
                        if take > 0 {
                            let _ = mp3.push(&self.pending[..take]);
                            if take < self.pending_len {
                                self.pending.copy_within(take..self.pending_len, 0);
                            }
                            self.pending_len -= take;
                        }
                    }

                    match mp3.next_frame(decoded_buf) {
                        Ok(Some(frame_info)) => {
                            let rate = frame_info.sample_rate;
                            let channels = frame_info.channels;
                            let samples = frame_info.samples();
                            if self.resampler.as_ref().map_or(true, |r| {
                                r.src_rate() != rate || r.src_channels() != channels
                            }) {
                                match Resampler::new(rate, channels) {
                                    Ok(r) => self.resampler = Some(r),
                                    Err(_) => {
                                        self.error = PlaybackError::UnsupportedFormat;
                                        break;
                                    }
                                }
                            }
                            let Some(resampler) = self.resampler.as_mut() else { break; };
                            let (consumed_frames, written_frames) =
                                resampler.process(&decoded_buf[..samples], out_pcm_16);
                            let consumed_samples = consumed_frames * resampler.src_channels() as usize;
                            if consumed_samples < samples {
                                self.decoded_pos = consumed_samples;
                                self.decoded_len = samples;
                            }
                            if written_frames > 0 {
                                scale_pcm_volume(&mut out_pcm_16[..written_frames * 2], self.volume);
                                self.out_pos = 0;
                                self.out_len = written_frames * 4;
                            }
                        }
                        Ok(None) => {
                            if self.file_eof {
                                if let Some(resampler) = self.resampler.as_mut() {
                                    let drained = resampler.drain(out_pcm_16);
                                    if drained > 0 {
                                        scale_pcm_volume(&mut out_pcm_16[..drained * 2], self.volume);
                                        self.out_pos = 0;
                                        self.out_len = drained * 4;
                                        continue;
                                    }
                                }
                                self.decoder_done = true;
                                break;
                            }
                            let read_buf = unsafe { &mut *core::ptr::addr_of_mut!(READ_SCRATCH) };
                            match std::fs_read(fd, read_buf) {
                                Ok(0) => {
                                    self.file_eof = true;
                                    mp3.finish();
                                }
                                Ok(n) => {
                                    let avail = mp3.available();
                                    let take = n.min(avail);
                                    if take > 0 {
                                        let _ = mp3.push(&read_buf[..take]);
                                    }
                                    if take < n {
                                        let leftover = n - take;
                                        let can_take = leftover.min(READ_BYTES - self.pending_len);
                                        self.pending[self.pending_len..self.pending_len + can_take]
                                            .copy_from_slice(&read_buf[take..take + can_take]);
                                        self.pending_len += can_take;
                                    }
                                }
                                Err(_) => {
                                    self.error = PlaybackError::ReadRefused;
                                    break;
                                }
                            }
                        }
                        Err(Mp3Error::Truncated) => {
                            self.file_eof = true;
                            self.decoder_done = true;
                            break;
                        }
                        Err(_) => {
                            self.error = PlaybackError::UnsupportedFormat;
                            break;
                        }
                    }
                }
                None => break,
            }
        }
        accepted_total
    }
}

/// Applies software volume scaling to interleaved 16-bit PCM samples.
fn scale_pcm_volume(samples: &mut [i16], volume: u8) {
    if volume >= 100 {
        return;
    }
    let vol = volume as i32;
    for s in samples.iter_mut() {
        let scaled = (*s as i32 * vol) / 100;
        *s = scaled.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_sniffing_distinguishes_wav_flac_and_mp3() {
        assert_eq!(
            sniff_format(b"RIFF\x24\x00\x00\x00WAVEfmt "),
            Some(AudioFormat::Wav)
        );
        assert_eq!(sniff_format(b"fLaC\x00\x00\x00\x22"), Some(AudioFormat::Flac));
        assert_eq!(sniff_format(b"ID3\x03\x00\x00\x00"), Some(AudioFormat::Mp3));
        assert_eq!(sniff_format(&[0xFF, 0xFB, 0x90, 0x64]), Some(AudioFormat::Mp3));
        assert_eq!(sniff_format(b"OggS\x00\x02"), None);
        assert_eq!(sniff_format(b"RIFF\x00\x00\x00\x00AVI "), None);
        assert_eq!(sniff_format(&[0, 1, 2, 3]), None);
        assert_eq!(sniff_format(b""), None);
    }

    #[test]
    fn a_stream_is_drained_only_when_every_written_frame_was_consumed() {
        assert!(!stream_drained(0, 0, false));
        assert!(!stream_drained(48_000, 47_999, false));
        assert!(!stream_drained(48_000, 48_000, true));
        assert!(stream_drained(48_000, 48_000, false));
        assert!(stream_drained(48_000, 48_100, false));
    }

    #[test]
    fn a_header_refusal_says_whether_the_file_or_its_samples_were_wrong() {
        assert_eq!(error_for_wav(WavError::NotRiff), PlaybackError::NotWav);
        assert_eq!(error_for_wav(WavError::NotWave), PlaybackError::NotWav);
        assert_eq!(error_for_wav(WavError::Truncated), PlaybackError::NotWav);
        assert_eq!(
            error_for_wav(WavError::UnsupportedFormat),
            PlaybackError::UnsupportedFormat
        );
    }

    #[test]
    fn every_refusal_has_something_the_screen_can_say() {
        for error in [
            PlaybackError::NoSource,
            PlaybackError::OpenRefused,
            PlaybackError::ReadRefused,
            PlaybackError::NotWav,
            PlaybackError::UnsupportedFormat,
            PlaybackError::StreamUnavailable,
            PlaybackError::WriteRefused,
        ] {
            assert!(!error.reason().is_empty());
        }
        assert_eq!(PlaybackError::None.reason(), "");
    }

    #[test]
    fn volume_scaling_clamps_and_scales_properly() {
        let mut samples = [1000i16, -1000i16, 30000i16, -30000i16];
        scale_pcm_volume(&mut samples, 100);
        assert_eq!(samples, [1000, -1000, 30000, -30000]);

        scale_pcm_volume(&mut samples, 50);
        assert_eq!(samples, [500, -500, 15000, -15000]);

        scale_pcm_volume(&mut samples, 0);
        assert_eq!(samples, [0, 0, 0, 0]);
    }

    #[test]
    fn duration_reporting_is_truthful_per_codec() {
        let mut player = Player::EMPTY;
        assert_eq!(player.duration_ms(), None);

        // WAV with 48000 frames at 48000 Hz = 1000 ms
        player.format = Some(AudioFormat::Wav);
        player.info = Some(WavInfo {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 16,
            data_offset: 44,
            data_len: 48_000 * 4,
        });
        assert_eq!(player.duration_ms(), Some(1000));

        // FLAC with 88200 total samples at 44100 Hz = 2000 ms
        player.format = Some(AudioFormat::Flac);
        player.flac_info = Some(StreamInfo {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            total_samples: 88_200,
        });
        assert_eq!(player.duration_ms(), Some(2000));

        // MP3 stream reports None truthfully (no fake duration)
        player.format = Some(AudioFormat::Mp3);
        assert_eq!(player.duration_ms(), None);
    }

    #[test]
    fn seek_refuses_when_not_holding_track() {
        let mut player = Player::EMPTY;
        let store = TrackStore::EMPTY;
        assert!(!player.seek(&store, 1000));
    }
}
