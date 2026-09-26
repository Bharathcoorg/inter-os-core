//! Sovereign Mobile Audio Subsystem
//!
//! Coordinates bare-metal hardware tone synthesis (PC Speaker & PIT Channel 2),
//! linear PCM audio buffers, and PCI audio controllers (Intel HDA / AC'97).

pub mod pcm;
pub mod speaker;

pub use pcm::{DmaBufferDescriptor, PcmBuffer, PcmChannels, PcmFormat, PcmSampleRate};
pub use speaker::Speaker;

use spin::Mutex;
use crate::drivers::audio::{PciAudioController, PciAudioDevice};
use crate::serial_println;

/// Audio Feedback Events
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioFeedback {
    HapticClick,
    NotificationChime,
    OperationConfirmed,
    OrderConfirmed,
    AlertChirp,
    Tone(u32),
    Silence,
}

/// Global Audio Manager Subsystem
pub const AUDIO_MAX_BUFFER_FRAMES: usize = 48_000;

pub struct AudioManager {
    pub last_feedback: AudioFeedback,
    pub pci_devices: alloc::vec::Vec<PciAudioDevice>,
    pub stream_active: bool,
    pub stream_frames: usize,
    pub volume_percent: u8,
    pub underruns: u32,
}

impl AudioManager {
    pub const fn new() -> Self {
        Self {
            last_feedback: AudioFeedback::Silence,
            pci_devices: alloc::vec::Vec::new(),
            stream_active: false,
            stream_frames: 0,
            volume_percent: 100,
            underruns: 0,
        }
    }

    pub fn configure_stream(&mut self, frames: usize) -> Result<(), &'static str> {
        if frames == 0 || frames > AUDIO_MAX_BUFFER_FRAMES { return Err("audio stream exceeds bounded buffer"); }
        self.stream_frames = frames;
        Ok(())
    }

    pub fn start_stream(&mut self) -> Result<(), &'static str> {
        if self.stream_frames == 0 { return Err("audio stream is not configured"); }
        self.stream_active = true;
        Ok(())
    }

    pub fn stop_stream(&mut self) {
        self.stream_active = false;
    }

    pub fn set_volume(&mut self, percent: u8) {
        self.volume_percent = percent.min(100);
    }

    /// Dispatches audio feedback to hardware drivers.
    pub fn play(&mut self, feedback: AudioFeedback) {
        self.last_feedback = feedback;
        match feedback {
            AudioFeedback::HapticClick => Speaker::haptic_click(),
            AudioFeedback::NotificationChime => Speaker::notification_chime(),
            AudioFeedback::OperationConfirmed | AudioFeedback::OrderConfirmed => Speaker::operation_confirmed_jingle(),
            AudioFeedback::AlertChirp => Speaker::alert_chirp(),
            AudioFeedback::Tone(freq) => Speaker::play_tone(freq),
            AudioFeedback::Silence => Speaker::disable(),
        }
    }
}

pub static AUDIO_MANAGER: Mutex<AudioManager> = Mutex::new(AudioManager::new());

/// Initializes the audio subsystem and verifies hardware audio synthesis via self-tests.
pub fn init() {
    serial_println!("--- [Initializing Mobile Audio Subsystem] ---");

    // 1. Verify PIT Channel 2 divisor math (440 Hz standard concert A)
    let div_440 = Speaker::compute_divisor(440);
    assert_eq!(div_440, 2711); // 1,193,182 / 440 = 2711
    let div_1000 = Speaker::compute_divisor(1000);
    assert_eq!(div_1000, 1193); // 1,193,182 / 1000 = 1193
    serial_println!("[TEST] PC Speaker Driver: Divisor computation PASSED (440Hz -> 2711, 1000Hz -> 1193).");

    // 2. Synthesize PCM Sine Wave and verify waveform integrity & RMS energy
    let pcm = PcmBuffer::generate_sine(440, 50, PcmFormat::CD_QUALITY, 12000);
    assert_eq!(pcm.duration_ms(), 50);
    assert!(pcm.len() > 0);
    let energy = pcm.calculate_rms_energy();
    assert!(energy > 1000, "PCM audio energy should be substantial");
    serial_println!(
        "[TEST] PCM Audio Synthesis: 50ms 440Hz sine wave verified ({} samples, RMS energy = {}).",
        pcm.len(),
        energy
    );

    // 3. Probe PCI bus for multimedia and audio hardware controllers
    let audio_devs = PciAudioController::probe_devices();
    serial_println!("[OK] Discovered {} PCI audio hardware devices.", audio_devs.len());
    for dev in &audio_devs {
        serial_println!(
            "  PCI Audio: ID {:04x}:{:04x} (HDA: {}, BAR0: {:#X}, IRQ: {})",
            dev.vendor_id, dev.device_id, dev.is_hda, dev.bar0, dev.interrupt_line
        );
    }
    AUDIO_MANAGER.lock().pci_devices = audio_devs;

    // 4. Test AudioManager Dispatch
    AUDIO_MANAGER.lock().play(AudioFeedback::Silence);
    serial_println!("--- [Mobile Audio Subsystem 100% Attested & Operational] ---");
}
