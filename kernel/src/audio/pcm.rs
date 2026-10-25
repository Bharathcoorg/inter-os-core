//! PCM Audio Buffer & Hardware DMA Descriptor Architecture
//!
//! Provides structured 16-bit linear PCM audio buffers, fixed-point waveform
//! synthesis (sine, square, triangle), and DMA Buffer Descriptor List (BDL)
//! mapping for PCI audio hardware controllers.

use alloc::vec::Vec;

pub const PCM_MAX_FRAMES: usize = 48_000;
pub const PCM_MAX_BYTES: usize = PCM_MAX_FRAMES * 2 * 2;

/// Audio Sample Rates
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PcmSampleRate {
    Hz8000 = 8000,
    Hz16000 = 16000,
    Hz22050 = 22050,
    Hz44100 = 44100,
    Hz48000 = 48000,
}

/// Audio Channel Mode
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PcmChannels {
    Mono = 1,
    Stereo = 2,
}

/// PCM Audio Format Descriptor
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: PcmSampleRate,
    pub channels: PcmChannels,
    pub bits_per_sample: u8,
}

impl PcmFormat {
    pub const CD_QUALITY: Self = Self {
        sample_rate: PcmSampleRate::Hz44100,
        channels: PcmChannels::Stereo,
        bits_per_sample: 16,
    };

    pub const VOICE_QUALITY: Self = Self {
        sample_rate: PcmSampleRate::Hz16000,
        channels: PcmChannels::Mono,
        bits_per_sample: 16,
    };
}

/// One AC'97 Buffer Descriptor List Entry (BDLE), which is exactly 8 bytes.
///
/// WHY this shape: the AC'97 2.3 buffer descriptor is a 32-bit physical address
/// followed by a 32-bit control word. The low 16 control bits are the buffer
/// length in 16-bit samples (not bytes and not frames), bit 30 is the Buffer
/// Underrun Policy, and bit 31 is Interrupt On Completion. The previous
/// three-field 12-byte record could not be handed to an AC'97 controller: the
/// hardware would have read the next entry's address as this entry's control
/// word.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DmaBufferDescriptor {
    /// 32-bit physical base address of the PCM buffer (low two bits ignored).
    pub addr: u32,
    /// Low 16 bits: length in 16-bit samples. Bits 30/31: BUP / IOC.
    pub control: u32,
}

/// Size of one AC'97 buffer descriptor on the wire.
pub const DMA_BDL_DESCRIPTOR_BYTES: usize = 8;

impl DmaBufferDescriptor {
    /// Interrupt On Completion: the controller sets BCIS after this entry.
    pub const IOC: u32 = 1 << 31;
    /// Buffer Underrun Policy: repeat the last sample instead of silence.
    pub const BUP: u32 = 1 << 30;

    /// Builds an AC'97 descriptor for `frames` interleaved frames.
    ///
    /// Refuses zero-length and lengths that cannot be expressed in the
    /// hardware's 16-bit sample counter.
    pub fn ac97(address: u32, frames: usize, channels: usize) -> Result<Self, &'static str> {
        if frames == 0 || channels == 0 {
            return Err("AC'97 descriptor length must be non-zero");
        }
        let samples = frames
            .checked_mul(channels)
            .ok_or("AC'97 descriptor length overflow")?;
        if samples > u16::MAX as usize {
            return Err("AC'97 descriptor length exceeds 16-bit sample count");
        }
        Ok(Self {
            addr: address & !0x3,
            control: samples as u32,
        })
    }

    pub fn with_ioc(mut self) -> Self {
        self.control |= Self::IOC;
        self
    }

    pub fn with_bup(mut self) -> Self {
        self.control |= Self::BUP;
        self
    }

    /// Number of 16-bit samples described by this entry.
    pub fn sample_count(&self) -> u16 {
        (self.control & 0xFFFF) as u16
    }

    /// Little-endian bytes as read by the controller's DMA engine.
    pub fn encode(&self) -> [u8; DMA_BDL_DESCRIPTOR_BYTES] {
        let mut out = [0u8; DMA_BDL_DESCRIPTOR_BYTES];
        out[0..4].copy_from_slice(&self.addr.to_le_bytes());
        out[4..8].copy_from_slice(&self.control.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8; DMA_BDL_DESCRIPTOR_BYTES]) -> Self {
        let mut addr = [0u8; 4];
        let mut control = [0u8; 4];
        addr.copy_from_slice(&bytes[0..4]);
        control.copy_from_slice(&bytes[4..8]);
        Self {
            addr: u32::from_le_bytes(addr),
            control: u32::from_le_bytes(control),
        }
    }
}

/// Linear 16-bit Signed PCM Audio Buffer
pub struct PcmBuffer {
    pub format: PcmFormat,
    pub samples: Vec<i16>,
}

impl PcmBuffer {
    /// Creates an empty PCM buffer with target format.
    pub fn new(format: PcmFormat) -> Self {
        Self {
            format,
            samples: Vec::new(),
        }
    }

    /// Total number of audio samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.format.channels as usize
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.format.bits_per_sample != 16 { return Err("unsupported PCM bit depth"); }
        if self.samples.len() > PCM_MAX_FRAMES * self.format.channels as usize { return Err("PCM buffer exceeds frame bound"); }
        Ok(())
    }

    pub fn byte_len(&self) -> usize {
        self.samples.len().saturating_mul(core::mem::size_of::<i16>())
    }

    pub fn encode_le(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        self.validate()?;
        let needed = self.byte_len();
        if out.len() < needed { return Err("PCM output buffer too small"); }
        for (i, sample) in self.samples.iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Ok(needed)
    }

    /// Duration in milliseconds based on sample rate and channel count.
    pub fn duration_ms(&self) -> u32 {
        let rate = self.format.sample_rate as u32;
        let channels = self.format.channels as u32;
        if rate == 0 || channels == 0 {
            return 0;
        }
        let total_frames = self.samples.len() as u32 / channels;
        (total_frames * 1000) / rate
    }

    /// Computes Root Mean Square (RMS) energy across all samples.
    pub fn calculate_rms_energy(&self) -> u32 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sum_sq: u64 = 0;
        for &s in &self.samples {
            sum_sq += (s as i64 * s as i64) as u64;
        }
        let mean = sum_sq / (self.samples.len() as u64);
        // Integer square root
        let mut x = mean;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + mean / x) / 2;
        }
        x as u32
    }

    /// Generates a synthetic sine wave tone using a fixed-point polynomial approximation.
    pub fn generate_sine(freq_hz: u32, duration_ms: u32, format: PcmFormat, amplitude: i16) -> Self {
        let sample_rate = format.sample_rate as u32;
        let channels = format.channels as u32;
        let total_frames = ((sample_rate as u64 * duration_ms as u64) / 1000).min(PCM_MAX_FRAMES as u64) as u32;
        let total_samples = (total_frames * channels) as usize;

        let mut samples = Vec::with_capacity(total_samples);

        // Fixed-point phase accumulator (32-bit: top 16 bits = fractional cycle 0..65535)
        let phase_inc = (((freq_hz as u64) << 32) / (sample_rate as u64)) as u32;
        let mut phase: u32 = 0;

        for _ in 0..total_frames {
            // Map phase [0..2^32-1] to quadrant [0..4] in Q15
            let cycle = (phase >> 16) as u16;
            let sample_val = Self::fast_sin(cycle, amplitude);

            for _ in 0..channels {
                samples.push(sample_val);
            }

            phase = phase.wrapping_add(phase_inc);
        }

        Self { format, samples }
    }

    /// Approximates sin(2*pi * phase/65536) using a 5th-order Taylor polynomial with integer math.
    fn fast_sin(phase: u16, amp: i16) -> i16 {
        // Normalize phase to [-16384, 16384] for range [-pi/2, pi/2]
        let p = (phase as i32) - 32768; // [-32768, 32767]
        let x = if p < -16384 {
            -32768 - p
        } else if p > 16384 {
            32768 - p
        } else {
            p
        };

        // x is in [-16384, 16384], normalized so 16384 = pi/2
        // sin(x) ≈ (4*x*(16384 - |x|)) / 65536
        let abs_x = x.abs();
        let num = 4 * x * (16384 - abs_x);
        let q15_sin = num / 16384; // roughly in [-16384, 16384]

        let scaled = (q15_sin * (amp as i32)) / 16384;
        scaled.clamp(-32768, 32767) as i16
    }
}

/// Default producer/consumer ring capacity: 16,384 frames (64 KiB stereo).
pub const PCM_STREAM_DEFAULT_FRAMES: usize = 16_384;

/// Errors from the bounded PCM stream.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PcmError {
    /// The requested format or capacity is outside what this stream supports.
    InvalidGeometry,
    /// The write length is empty or splits an interleaved frame.
    InvalidLength,
    /// The ring has no free frame.
    Full,
    /// The ring has no queued frame.
    Empty,
}

/// A bounded single-producer/single-consumer PCM ring buffer.
///
/// The buffer is allocated once with a validated frame capacity and never
/// grows. `write_bytes` accepts whole interleaved frames up to the free space
/// (a partial write) and returns [`PcmError::Full`] rather than dropping data
/// when nothing fits. `read_into` is the consumer side used by the AC'97
/// controller, and the counters distinguish frames accepted, frames handed to
/// the controller, and times the device went idle with an empty queue after
/// playback had started (counted as underruns).
///
/// WHY `frames_consumed` is not "frames heard": it counts frames copied from
/// this ring into the controller's DMA memory. The controller may still be
/// playing them, and the platform has no way to measure the speaker output.
pub struct PcmStream {
    format: PcmFormat,
    buffer: Vec<u8>,
    capacity_frames: usize,
    read_frame: usize,
    write_frame: usize,
    queued_frames: usize,
    frames_written: u64,
    frames_consumed: u64,
    underruns: u32,
    playback_started: bool,
}

impl PcmStream {
    /// Creates a stream for 16-bit PCM with a fixed, validated frame capacity.
    pub fn new(format: PcmFormat, capacity_frames: usize) -> Result<Self, PcmError> {
        if format.bits_per_sample != 16
            || !matches!(format.channels, PcmChannels::Mono | PcmChannels::Stereo)
            || capacity_frames == 0
            || capacity_frames > PCM_MAX_FRAMES
        {
            return Err(PcmError::InvalidGeometry);
        }
        let frame_bytes = format.channels as usize * 2;
        let bytes = capacity_frames
            .checked_mul(frame_bytes)
            .ok_or(PcmError::InvalidGeometry)?;
        Ok(Self {
            format,
            buffer: alloc::vec![0u8; bytes],
            capacity_frames,
            read_frame: 0,
            write_frame: 0,
            queued_frames: 0,
            frames_written: 0,
            frames_consumed: 0,
            underruns: 0,
            playback_started: false,
        })
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }

    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    pub fn queued_frames(&self) -> usize {
        self.queued_frames
    }

    pub fn free_frames(&self) -> usize {
        self.capacity_frames - self.queued_frames
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    pub fn frames_consumed(&self) -> u64 {
        self.frames_consumed
    }

    pub fn underruns(&self) -> u32 {
        self.underruns
    }

    /// Whether at least one frame has been handed to the device. An underrun
    /// before playback starts is not an underrun: nothing was playing.
    pub fn playback_started(&self) -> bool {
        self.playback_started
    }

    fn frame_bytes(&self) -> usize {
        self.format.channels as usize * (self.format.bits_per_sample as usize / 8)
    }

    /// Enqueues whole interleaved frames, accepting what fits.
    ///
    /// Returns the number of **bytes** accepted. A caller that supplies more
    /// frames than the ring can hold gets a short count (never a fabricated
    /// success), and a caller that supplies none gets [`PcmError::Full`].
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<usize, PcmError> {
        let frame_bytes = self.frame_bytes();
        if bytes.is_empty() || bytes.len() % frame_bytes != 0 {
            return Err(PcmError::InvalidLength);
        }
        let free = self.free_frames();
        if free == 0 {
            return Err(PcmError::Full);
        }
        let requested_frames = bytes.len() / frame_bytes;
        let frames = requested_frames.min(free);
        for index in 0..frames {
            let src = index * frame_bytes;
            let dst = self.write_frame * frame_bytes;
            self.buffer[dst..dst + frame_bytes]
                .copy_from_slice(&bytes[src..src + frame_bytes]);
            self.write_frame += 1;
            if self.write_frame == self.capacity_frames {
                self.write_frame = 0;
            }
        }
        self.queued_frames += frames;
        self.frames_written = self.frames_written.saturating_add(frames as u64);
        Ok(frames * frame_bytes)
    }

    /// Dequeues up to `max_frames` whole frames into `out`.
    ///
    /// Returns the number of frames copied and counts them as consumed (handed
    /// to the controller). It does not claim they were heard.
    pub fn read_into(&mut self, out: &mut [u8], max_frames: usize) -> usize {
        let frame_bytes = self.frame_bytes();
        if frame_bytes == 0 {
            return 0;
        }
        let frames = max_frames
            .min(self.queued_frames)
            .min(out.len() / frame_bytes);
        if frames == 0 {
            return 0;
        }
        for index in 0..frames {
            let src = self.read_frame * frame_bytes;
            let dst = index * frame_bytes;
            out[dst..dst + frame_bytes]
                .copy_from_slice(&self.buffer[src..src + frame_bytes]);
            self.read_frame += 1;
            if self.read_frame == self.capacity_frames {
                self.read_frame = 0;
            }
        }
        self.queued_frames -= frames;
        self.frames_consumed = self.frames_consumed.saturating_add(frames as u64);
        self.playback_started = true;
        frames
    }

    /// Records one underrun: the controller went idle with an empty queue
    /// after playback had started.
    ///
    /// Returns `false` when nothing has been played yet: an idle device before
    /// the first batch is not evidence that the producer fell behind.
    pub fn record_underrun(&mut self) -> bool {
        if !self.playback_started {
            return false;
        }
        self.underruns = self.underruns.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEREO_16: PcmFormat = PcmFormat {
        sample_rate: PcmSampleRate::Hz48000,
        channels: PcmChannels::Stereo,
        bits_per_sample: 16,
    };

    #[test]
    fn ac97_descriptor_encodes_samples_not_bytes() {
        let descriptor = DmaBufferDescriptor::ac97(0x1_2345, 512, 2)
            .expect("valid descriptor")
            .with_ioc();
        // 512 stereo frames = 1024 16-bit samples in the low control word.
        assert_eq!(descriptor.sample_count(), 1024);
        assert_eq!(descriptor.addr, 0x1_2344, "low two address bits are ignored");
        assert_eq!(descriptor.control & DmaBufferDescriptor::IOC, DmaBufferDescriptor::IOC);
        assert_eq!(descriptor.control & DmaBufferDescriptor::BUP, 0);

        let bytes = descriptor.encode();
        assert_eq!(&bytes[0..4], &0x1_2344u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &(1024u32 | DmaBufferDescriptor::IOC).to_le_bytes());
        assert_eq!(DmaBufferDescriptor::decode(&bytes), descriptor);
    }

    #[test]
    fn ac97_descriptor_refuses_zero_and_oversized_lengths() {
        assert!(DmaBufferDescriptor::ac97(0x1000, 0, 2).is_err());
        assert!(DmaBufferDescriptor::ac97(0x1000, 1, 0).is_err());
        // 32768 stereo frames = 65536 samples, one past the 16-bit counter.
        assert!(DmaBufferDescriptor::ac97(0x1000, 32_768, 2).is_err());
    }

    #[test]
    fn partial_write_fills_only_free_frames() {
        let mut stream = PcmStream::new(STEREO_16, 4).expect("stream");
        // 4 frames fit in total; write 3 frames.
        assert_eq!(stream.write_bytes(&[0u8; 12]).unwrap(), 12);
        assert_eq!(stream.queued_frames(), 3);
        assert_eq!(stream.free_frames(), 1);
        // A 2-frame write is shortened to the one frame that fits.
        assert_eq!(stream.write_bytes(&[1u8; 8]).unwrap(), 4);
        assert_eq!(stream.queued_frames(), 4);
        assert_eq!(stream.frames_written(), 4);
    }

    #[test]
    fn full_buffer_refuses_more_data() {
        let mut stream = PcmStream::new(STEREO_16, 2).expect("stream");
        assert_eq!(stream.write_bytes(&[0u8; 8]).unwrap(), 8);
        assert_eq!(stream.write_bytes(&[0u8; 4]), Err(PcmError::Full));
        assert_eq!(stream.frames_written(), 2);
    }

    #[test]
    fn frame_accounting_tracks_written_and_consumed() {
        let mut stream = PcmStream::new(STEREO_16, 8).expect("stream");
        assert_eq!(stream.write_bytes(&[0xAAu8; 32]).unwrap(), 32);
        assert_eq!(stream.frames_written(), 8);
        assert_eq!(stream.frames_consumed(), 0);

        let mut out = [0u8; 24];
        assert_eq!(stream.read_into(&mut out, 6), 6);
        assert_eq!(stream.frames_consumed(), 6);
        assert_eq!(stream.queued_frames(), 2);
        assert!(stream.playback_started());

        let mut rest = [0u8; 16];
        assert_eq!(stream.read_into(&mut rest, 16), 2);
        assert_eq!(stream.frames_consumed(), 8);
        assert_eq!(stream.queued_frames(), 0);
    }

    #[test]
    fn ring_wraps_without_losing_order() {
        let mut stream = PcmStream::new(STEREO_16, 2).expect("stream");
        // Each frame is 4 bytes: frame 0 = 00 00, frame 1 = 01 01.
        stream.write_bytes(&[0, 0, 0, 0, 1, 1, 1, 1]).unwrap();
        let mut first = [0u8; 8];
        assert_eq!(stream.read_into(&mut first, 2), 2);
        assert_eq!(&first[0..4], &[0, 0, 0, 0]);
        assert_eq!(&first[4..8], &[1, 1, 1, 1]);

        stream.write_bytes(&[2, 2, 2, 2, 3, 3, 3, 3]).unwrap();
        let mut second = [0u8; 8];
        assert_eq!(stream.read_into(&mut second, 2), 2);
        assert_eq!(&second[0..4], &[2, 2, 2, 2]);
        assert_eq!(&second[4..8], &[3, 3, 3, 3]);
    }

    #[test]
    fn underrun_requires_playback_to_have_started() {
        let mut stream = PcmStream::new(STEREO_16, 4).expect("stream");
        assert!(!stream.record_underrun());
        assert_eq!(stream.underruns(), 0);

        stream.write_bytes(&[0u8; 8]).unwrap();
        let mut out = [0u8; 8];
        assert_eq!(stream.read_into(&mut out, 2), 2);
        assert!(stream.record_underrun());
        assert!(stream.record_underrun());
        assert_eq!(stream.underruns(), 2);
    }

    #[test]
    fn write_rejects_split_frames_and_empty_input() {
        let mut stream = PcmStream::new(STEREO_16, 4).expect("stream");
        assert_eq!(stream.write_bytes(&[]), Err(PcmError::InvalidLength));
        assert_eq!(stream.write_bytes(&[0u8; 3]), Err(PcmError::InvalidLength));
    }

    #[test]
    fn capacity_is_bounded_at_creation() {
        assert_eq!(PcmStream::new(STEREO_16, 0).err(), Some(PcmError::InvalidGeometry));
        assert_eq!(
            PcmStream::new(STEREO_16, PCM_MAX_FRAMES + 1).err(),
            Some(PcmError::InvalidGeometry)
        );
    }
}
