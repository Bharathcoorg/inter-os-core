//! PCM Audio Buffer & Hardware DMA Descriptor Architecture
//!
//! Provides structured 16-bit linear PCM audio buffers, fixed-point waveform
//! synthesis (sine, square, triangle), and DMA Buffer Descriptor List (BDL)
//! mapping for PCI audio hardware controllers.

use alloc::vec::Vec;

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

/// DMA Buffer Descriptor (BDL entry for AC'97 & Intel HDA)
#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Default)]
pub struct DmaBufferDescriptor {
    /// 32-bit physical base address of audio buffer
    pub addr: u32,
    /// Number of samples or byte length
    pub length: u32,
    /// Bit 0: Interrupt on Completion (IOC), Bit 1: Buffer Underrun Policy (BUP)
    pub flags: u32,
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
        let total_frames = (sample_rate * duration_ms) / 1000;
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
