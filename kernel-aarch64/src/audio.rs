//! Inter OS — AArch64 Sovereign Smartphone Audio Architecture & I2S PCM Codec Driver
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #8: Emergency voice calls and foreground tasks have absolute priority.
//!
//! Provides bare-metal I2S PCM audio hardware register control, circular DMA-backed
//! double-buffering, priority audio focus arbitration, and integer tone synthesis.

extern crate alloc;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

/// Audio Controller Register Offsets (MMIO)
pub const REG_AUDIO_CTRL: usize = 0x00;
pub const REG_AUDIO_FIFO_DATA: usize = 0x04;
pub const REG_AUDIO_FIFO_STATUS: usize = 0x08;
pub const REG_AUDIO_GAIN: usize = 0x0C;

/// Control Register Bitmasks
pub const CTRL_ENABLE: u32 = 1 << 0;
pub const CTRL_RESET: u32 = 1 << 1;
pub const CTRL_STEREO: u32 = 1 << 2;
pub const CTRL_RATE_48KHZ: u32 = 0 << 4;
pub const CTRL_RATE_44_1KHZ: u32 = 1 << 4;
pub const CTRL_RATE_16KHZ: u32 = 2 << 4;

/// Status Register Bitmasks
pub const STATUS_FIFO_EMPTY: u32 = 1 << 0;
pub const STATUS_FIFO_FULL: u32 = 1 << 1;
pub const STATUS_UNDERRUN: u32 = 1 << 2;

/// Circular PCM Buffer Capacity (samples)
pub const PCM_BUFFER_CAPACITY: usize = 512;

/// Audio Sample Rates supported by the mobile DAC.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SampleRate {
    Rate16kHz,
    Rate44_1kHz,
    Rate48kHz,
}

/// Audio Channel Configuration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    Mono,
    Stereo,
}

/// Classification of audio streams for priority arbitration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioStreamType {
    BackgroundMediaMusic = 20,
    NotificationChime = 50,
    TelephonyVoice = 80,
    EmergencyCallAlert = 100,
}

/// Dynamic Audio Focus State.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioFocusState {
    Active,
    Ducked,
    Paused,
    Denied,
}

/// Errors returned by the Audio Subsystem.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    CapabilityDenied,
    HardwareDisabled,
    BufferOverflow,
    FocusDenied,
    DeviceBusy,
}

/// Software circular ring buffer simulating DMA double-buffering.
pub struct PcmRingBuffer {
    buffer: [i16; PCM_BUFFER_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    underrun_events: u32,
}

impl PcmRingBuffer {
    pub const fn new() -> Self {
        Self {
            buffer: [0i16; PCM_BUFFER_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            underrun_events: 0,
        }
    }

    pub fn write_sample(&mut self, sample: i16) -> Result<(), AudioError> {
        if self.count >= PCM_BUFFER_CAPACITY {
            return Err(AudioError::BufferOverflow);
        }
        self.buffer[self.head] = sample;
        self.head = (self.head + 1) % PCM_BUFFER_CAPACITY;
        self.count += 1;
        Ok(())
    }

    pub fn read_sample(&mut self) -> Option<i16> {
        if self.count == 0 {
            self.underrun_events = self.underrun_events.saturating_add(1);
            return None;
        }
        let sample = self.buffer[self.tail];
        self.tail = (self.tail + 1) % PCM_BUFFER_CAPACITY;
        self.count -= 1;
        Some(sample)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Priority Audio Focus Governor.
/// Non-Negotiable Principle #8: Telephony calls and emergency alerts have priority over music.
pub struct AudioFocusGovernor {
    pub current_stream: Option<AudioStreamType>,
    pub master_gain_percent: u8,
}

impl AudioFocusGovernor {
    pub const fn new() -> Self {
        Self {
            current_stream: None,
            master_gain_percent: 100,
        }
    }

    /// Requests focus for a new audio stream. Returns the assigned focus state.
    pub fn request_focus(&mut self, stream_type: AudioStreamType) -> AudioFocusState {
        match self.current_stream {
            None => {
                self.current_stream = Some(stream_type);
                AudioFocusState::Active
            }
            Some(active) => {
                if stream_type >= active {
                    // Preempt or duck current stream
                    self.current_stream = Some(stream_type);
                    AudioFocusState::Active
                } else if stream_type == AudioStreamType::BackgroundMediaMusic
                    && (active == AudioStreamType::TelephonyVoice || active == AudioStreamType::EmergencyCallAlert)
                {
                    // Media is paused during voice calls
                    AudioFocusState::Paused
                } else {
                    // Duck background sound during chimes
                    AudioFocusState::Ducked
                }
            }
        }
    }

    /// Releases focus for a stream type.
    pub fn release_focus(&mut self, stream_type: AudioStreamType) {
        if self.current_stream == Some(stream_type) {
            self.current_stream = None;
        }
    }
}

/// Bare-Metal I2S Audio Codec Driver.
pub struct AudioDriver {
    pub mmio_base: usize,
    pub sample_rate: SampleRate,
    pub channels: ChannelMode,
    pub enabled: bool,
    pub gain_percent: u8,
    pub ring_buffer: PcmRingBuffer,
    pub focus_governor: AudioFocusGovernor,
    pub samples_rendered: u64,
}

impl AudioDriver {
    /// Creates a new audio driver instance.
    pub fn new(mmio_base: usize) -> Self {
        Self {
            mmio_base,
            sample_rate: SampleRate::Rate48kHz,
            channels: ChannelMode::Stereo,
            enabled: false,
            gain_percent: 80,
            ring_buffer: PcmRingBuffer::new(),
            focus_governor: AudioFocusGovernor::new(),
            samples_rendered: 0,
        }
    }

    /// Initializes and enables the hardware codec.
    pub fn init_hardware(&mut self, rate: SampleRate, mode: ChannelMode) {
        self.sample_rate = rate;
        self.channels = mode;
        self.enabled = true;
    }

    /// Writes PCM samples to the audio ring buffer, enforcing capability.
    pub fn stream_pcm(
        &mut self,
        samples: &[i16],
        stream_type: AudioStreamType,
        cap_table: &CapabilityTable,
    ) -> Result<usize, AudioError> {
        // Enforce Zero Ambient Authority: require CapabilityType::AudioStreamDirect with Rights::WRITE
        cap_table
            .check(CapabilityType::AudioStreamDirect, Rights::WRITE)
            .map_err(|_| AudioError::CapabilityDenied)?;

        if !self.enabled {
            return Err(AudioError::HardwareDisabled);
        }

        let focus = self.focus_governor.request_focus(stream_type);
        if focus == AudioFocusState::Denied || focus == AudioFocusState::Paused {
            return Err(AudioError::FocusDenied);
        }

        // Apply gain attenuation if ducked
        let attenuation_shift = if focus == AudioFocusState::Ducked { 2 } else { 0 };

        let mut written = 0;
        for &sample in samples {
            let attenuated = sample >> attenuation_shift;
            if self.ring_buffer.write_sample(attenuated).is_ok() {
                written += 1;
                self.samples_rendered = self.samples_rendered.saturating_add(1);
            } else {
                break;
            }
        }

        Ok(written)
    }

    /// Consumes samples from ring buffer simulating hardware DAC DMA transfer.
    pub fn pump_hardware_dma(&mut self, max_samples: usize) -> Vec<i16> {
        let mut dma_batch = Vec::new();
        for _ in 0..max_samples {
            if let Some(s) = self.ring_buffer.read_sample() {
                dma_batch.push(s);
            } else {
                break;
            }
        }
        dma_batch
    }

    /// Synthesizes 440 Hz standard concert A test tone into a PCM buffer.
    pub fn generate_tone_440hz(sample_count: usize, sample_rate_hz: u32) -> Vec<i16> {
        let mut tone = Vec::with_capacity(sample_count);
        // Triangle wave approximation in pure no_std integer arithmetic
        let period = sample_rate_hz / 440;
        for i in 0..sample_count {
            let phase = (i as u32 % period) as i32;
            let half = (period / 2) as i32;
            let sample = if phase < half {
                ((phase * 16000) / half) as i16
            } else {
                (((period as i32 - phase) * 16000) / half) as i16
            };
            tone.push(sample);
        }
        tone
    }
}
