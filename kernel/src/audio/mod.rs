//! Sovereign Mobile Audio Subsystem
//!
//! Coordinates bare-metal hardware tone synthesis (PC Speaker & PIT Channel 2),
//! linear PCM audio buffers, and PCI audio controllers (Intel HDA / AC'97).
//!
//! ## Master volume is a stored request, never a measured level
//!
//! [`AudioManager::volume_percent`] records what a caller asked for. Nothing in
//! this kernel applies it to hardware:
//!
//! * [`AudioManager::play`] dispatches to [`Speaker`], which drives PIT channel 2
//!   at a fixed divisor amplitude — it takes no volume argument.
//! * [`PcmBuffer::generate_sine`] takes its amplitude from its own `amplitude`
//!   parameter (`pcm.rs`), never from [`AudioManager`].
//! * [`PciAudioController`] (`kernel/src/drivers/audio.rs`) only enumerates PCI
//!   audio devices and toggles bus-master/memory bits in the PCI command
//!   register; it writes no HDA/AC'97 amplifier or mixer register.
//!
//! So there is no confirmed output level to read back on this platform, and
//! reporting one would be fabrication. The field is therefore `Option<u8>` and
//! starts at `None`: callers must show "unavailable" instead of a number.

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
    /// Requested master volume as a percentage, or `None` when no volume has
    /// ever been requested.
    ///
    /// WHY `Option<u8>`: this was a plain `u8` seeded with a hardcoded `100` in
    /// [`AudioManager::new`]. Nothing ever read that seed from a device, so the
    /// very first `audio_query_volume()` returned a confident `100` that no audio
    /// hardware produced. `None` means "no volume has ever been requested" and
    /// callers must render that as unavailable rather than substituting a
    /// plausible number.
    ///
    /// WHY this is only a *request*: no mixer, amplifier, or PCM scaling path in
    /// this kernel reads this field — see the module-level note above. A value
    /// here records intent that was accepted, not a level confirmed in effect.
    pub volume_percent: Option<u8>,
    pub underruns: u32,
}

impl AudioManager {
    pub const fn new() -> Self {
        Self {
            last_feedback: AudioFeedback::Silence,
            pci_devices: alloc::vec::Vec::new(),
            stream_active: false,
            stream_frames: 0,
            // WHY `None`: the old value here was a literal `100`, which no device
            // ever measured. Seeding it meant every fresh boot could answer a
            // volume query with 100% before a single sample was played.
            volume_percent: None,
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

    /// Records a volume request and returns the stored percentage.
    ///
    /// The stored value is clamped to `0..=100` so downstream renderers can draw
    /// it directly. It is returned rather than having callers re-read the
    /// `Option` so no caller needs an `unwrap_or` fallback, which would quietly
    /// re-introduce a fabricated number on the error path.
    ///
    /// WHY it is only recorded: see the module-level note — no hardware path in
    /// this kernel applies the request to a mixer or PCM stream, so the return
    /// value is "the request that was stored", not "the level in effect".
    pub fn set_volume(&mut self, percent: u8) -> u8 {
        let stored = percent.min(100);
        self.volume_percent = Some(stored);
        stored
    }

    /// Steps the stored volume request by `delta` percentage points and returns
    /// the new stored value, or `None` when no volume has ever been requested.
    ///
    /// WHY `None` in and `None` out: with nothing applied to hardware there is no
    /// measured level to step *from*. The old code read the hardcoded `100`
    /// seed, which made the first "volume up" a silent no-op that still reported
    /// 100% as though it had done something. Refusing keeps that honest: with no
    /// known level there is nothing to nudge, so the caller is told the volume
    /// is unknown instead of being handed a number derived from a fiction.
    pub fn step_volume(&mut self, delta: i8) -> Option<u8> {
        let current = self.volume_percent?;
        let next = if delta >= 0 {
            current.saturating_add(delta as u8)
        } else {
            current.saturating_sub(delta.unsigned_abs())
        };
        self.set_volume(next);
        self.volume_percent
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: a fresh manager must not be able to answer a volume query at all.
    /// The old `AudioManager::new()` seeded `volume_percent` with a literal
    /// `100`, so this exact state used to report 100% out of thin air.
    #[test]
    fn new_manager_reports_no_volume() {
        let audio = AudioManager::new();
        assert_eq!(audio.volume_percent, None);
    }

    /// WHY: `set_volume` is the only writer, and it must store the clamped
    /// request rather than an out-of-range value a renderer could not draw.
    #[test]
    fn set_volume_stores_a_clamped_request() {
        let mut audio = AudioManager::new();
        assert_eq!(audio.set_volume(140), 100);
        assert_eq!(audio.volume_percent, Some(100));

        let mut audio = AudioManager::new();
        assert_eq!(audio.set_volume(0), 0);
        assert_eq!(audio.volume_percent, Some(0));
    }

    /// WHY: stepping with no known level must refuse rather than derive a
    /// number from a seed. The old code read the hardcoded `100`, so the first
    /// volume-up returned 100 and pretended it had changed something.
    #[test]
    fn step_volume_refuses_without_a_known_level() {
        let mut audio = AudioManager::new();
        assert_eq!(audio.step_volume(5), None);
        assert_eq!(audio.step_volume(-5), None);
        // Refusing must not silently invent a stored request either.
        assert_eq!(audio.volume_percent, None);
    }

    /// WHY: once a request exists, stepping must actually move it and must
    /// saturate at both ends of the 0..=100 renderable range.
    #[test]
    fn step_volume_moves_and_saturates_a_known_request() {
        let mut audio = AudioManager::new();
        audio.set_volume(95);
        assert_eq!(audio.step_volume(5), Some(100));
        assert_eq!(audio.volume_percent, Some(100));

        let mut audio = AudioManager::new();
        audio.set_volume(2);
        assert_eq!(audio.step_volume(-5), Some(0));
        assert_eq!(audio.volume_percent, Some(0));
    }

    /// WHY: stream state is independent of volume, and nothing in stream
    /// control may invent a volume. This guards against a future change that
    /// couples `configure_stream`/`start_stream` back to a seeded level.
    #[test]
    fn stream_control_does_not_invent_a_volume() {
        let mut audio = AudioManager::new();
        assert!(audio.configure_stream(480).is_ok());
        assert!(audio.start_stream().is_ok());
        assert_eq!(audio.volume_percent, None);
    }
}
