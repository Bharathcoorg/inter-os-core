//! Real playback of an imported WAV over the platform's AC'97 stream.
//!
//! WHAT this is: the player reads the file the track was imported from, turns
//! its samples into the one PCM format the controller accepts, and hands them
//! to the kernel's bounded stream ring. The position the screen shows is the
//! number of frames the device took, read back from the stream, not a clock.
//!
//! WHY the conversion happens here and not in the kernel: the controller is
//! programmed for 48 kHz stereo 16-bit and does not resample. A file at any
//! other rate would be noise if it were handed over unchanged, so the player
//! converts before it writes and refuses a format it cannot decode.

use interos_std as std;
use interos_std::wav::{parse_wav, PcmConverter, WavError, WavInfo};
use std::AudioStreamStatus;

use crate::{library, position_millis, TrackStore, SOURCE_MAX};

/// Source bytes read from the file in one step.
///
/// WHY this size: the widest expansion the converter can produce is an 8-bit
/// mono file at the lowest accepted rate, which becomes six output frames per
/// source frame. Two thousand and forty-eight source bytes cover 12,288 output
/// frames, and [`PCM_BYTES`] holds exactly those.
pub const READ_BYTES: usize = 2_048;

/// Converted bytes handed to the stream in one step.
pub const PCM_BYTES: usize = 49_152;

/// Where the player is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayerState {
    Stopped,
    Playing,
    Paused,
    Ended,
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
    /// The file is not a RIFF/WAVE container.
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

/// The read buffer shared by the header read and the streaming loop.
///
/// WHY a static and not a field: the event loop cannot allocate, and these
/// buffers are the size of the stream's own working set rather than of the
/// player's state. They are touched only by [`Player`]'s methods.
static mut READ_SCRATCH: [u8; READ_BYTES] = [0; READ_BYTES];
static mut PCM_SCRATCH: [u8; PCM_BYTES] = [0; PCM_BYTES];

/// The syscall code for a full ring: the caller should retry later.
const PIPE_FULL: i64 = -28;

/// One open track and the stream it is playing over.
///
/// WHAT this owns: the file descriptor the samples come from, the converter
/// that turns them into the controller's format, and the last status the
/// device reported. Every fact the screen shows comes from one of those.
pub struct Player {
    pub state: PlayerState,
    pub error: PlaybackError,
    /// The store index of the track this player opened.
    pub track: usize,
    /// The header of the file being played, once it has been read.
    pub info: Option<WavInfo>,
    /// The last status the device reported for this stream.
    pub status: Option<AudioStreamStatus>,
    /// Software volume applied to the samples the player hands over.
    pub volume: u8,
    converter: Option<PcmConverter>,
    fd: Option<u32>,
    cap: u64,
    stream: bool,
    /// Sample bytes the file still holds after the bytes already read.
    data_remaining: u64,
    /// Source bytes read but not yet converted, carried across steps.
    pending: [u8; READ_BYTES],
    pending_len: usize,
    /// Converted bytes waiting for room in the ring.
    out_len: usize,
    out_pos: usize,
}

impl Player {
    pub const EMPTY: Player = Player {
        state: PlayerState::Stopped,
        error: PlaybackError::None,
        track: 0,
        info: None,
        status: None,
        volume: 100,
        converter: None,
        fd: None,
        cap: 0,
        stream: false,
        data_remaining: 0,
        pending: [0; READ_BYTES],
        pending_len: 0,
        out_len: 0,
        out_pos: 0,
    };

    /// What the screen may say when the player refused the current track.
    pub fn error_reason(&self) -> &'static str {
        self.error.reason()
    }

    /// Whether this player has a file open for the given track.
    pub fn holds(&self, track: usize) -> bool {
        self.converter.is_some() && self.track == track && self.state != PlayerState::Stopped
    }

    /// Points the player at the track's new position after a reorder.
    ///
    /// WHY this and not a restart: the open file is the same file, so a queue
    /// that names it from a different position does not change what plays.
    pub fn retrack(&mut self, track: usize) {
        self.track = track;
    }

    /// The position the device has taken from this stream, in milliseconds.
    pub fn position_ms(&self) -> Option<u64> {
        self.status.map(|s| position_millis(s.frames_consumed))
    }

    /// The length the file's header claims, in milliseconds.
    ///
    /// WHY this is a claim: the header says how many sample frames the data
    /// chunk holds. It is the length of the file, not a measurement of the
    /// sound the speaker produced.
    pub fn duration_ms(&self) -> Option<u64> {
        self.info.map(|info| info.duration_ms())
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

        // A header may be longer than the bytes one read brought in. The file
        // has no seek, so the bytes before the samples are read and dropped.
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
        // Samples the header read already brought in are kept, not re-read:
        // the descriptor is sequential and cannot go back for them.
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
        if self.state == PlayerState::Paused && self.converter.is_some() {
            self.state = PlayerState::Playing;
            self.pump();
        }
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
        self.state = PlayerState::Ended;
    }

    /// Polls the device and returns what it reported.
    pub fn poll(&mut self) -> Option<AudioStreamStatus> {
        if self.stream {
            match std::audio_stream_status(self.cap) {
                Ok(status) => self.status = Some(status),
                Err(code) => {
                    // A refused status read would otherwise look exactly like a
                    // track that never ends, so say it once.
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
        if self.state != PlayerState::Playing || self.converter.is_none() {
            return;
        }
        self.pump();
        self.poll();
        if self.data_remaining == 0
            && self.pending_len == 0
            && self.out_pos >= self.out_len
        {
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
                match std::audio_stream_write(self.cap, &pcm[self.out_pos..self.out_len]) {
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
            if self.pending_len == 0 {
                if self.data_remaining == 0 {
                    break;
                }
                let want = (READ_BYTES as u64).min(self.data_remaining) as usize;
                match std::fs_read(fd, &mut self.pending[..want]) {
                    Ok(0) => {
                        // The file ended before the length its header claimed.
                        // The samples already played are real; there are no
                        // more, and that is not an error to report as one.
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
            let (consumed, written) = converter.convert(&self.pending[..self.pending_len], pcm);
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
        accepted_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_drained_only_when_every_written_frame_was_consumed() {
        // WHY: the ring holds about a third of a second. Treating the last
        // write as the end would stop the track while its tail was still
        // queued, and treating an empty stream as drained would end a track
        // before a single frame played.
        assert!(!stream_drained(0, 0, false));
        assert!(!stream_drained(48_000, 47_999, false));
        // The last batch is counted as consumed when it is submitted, but the
        // device is still playing it; ending here would truncate the track.
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
}
