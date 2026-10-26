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
//!   register; it writes no HDA amplifier or mixer register.
//! * The AC'97 PCM path ([`Ac97Controller`]) does write the codec's master and
//!   PCM-out volume registers to 0 dB, but it never reads back a measured
//!   level. Those writes unmute the codec; they do not establish a volume the
//!   manager can report.
//!
//! So there is no confirmed output level to read back on this platform, and
//! reporting one would be fabrication. The field is therefore `Option<u8>` and
//! starts at `None`: callers must show "unavailable" instead of a number.

pub mod pcm;
pub mod speaker;

pub use pcm::{
    DmaBufferDescriptor, PcmBuffer, PcmChannels, PcmError, PcmFormat, PcmSampleRate, PcmStream,
    PCM_STREAM_DEFAULT_FRAMES,
};
pub use speaker::Speaker;

// WHY `crate::sync::Mutex` and not `spin::Mutex`: the PCM open/write/status
// calls run in the preemptible syscall body and hold this lock across port I/O
// and DMA descriptor programming. A plain spinlock held by a task the scheduler
// switched out would never be released, so the critical section must defer
// preemption for its whole lifetime.
use crate::sync::Mutex;
use crate::drivers::audio::{
    Ac97Controller, Ac97Error, Ac97Status, PortAc97Io, AC97_SAMPLE_RATE_HZ,
};
use crate::drivers::audio::{PciAudioController, PciAudioDevice};
use crate::serial_println;

/// Outcome of a PCM stream operation, kept separate from the driver's typed
/// errors so the syscall layer can map each case to one ABI status.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioStreamError {
    /// No AC'97 controller is present or its codec did not initialize.
    Unavailable,
    /// The device exists but no stream is open.
    NotOpen,
    /// The bounded ring has no free frame.
    Full,
    /// The requested format is not 16-bit stereo 48 kHz.
    InvalidFormat,
    /// A DMA or register operation on the controller failed.
    DeviceFault,
}

/// Userspace-facing snapshot of the PCM path. Every field is measured or is
/// device state, except `sample_rate`, which is the programmed rate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamStatus {
    pub device_present: bool,
    pub device_running: bool,
    pub stream_open: bool,
    pub frames_written: u64,
    pub frames_consumed: u64,
    pub queued_frames: u64,
    /// Times the controller went idle with nothing queued after playback had
    /// started. This is inferred from the descriptor-completion bit and the
    /// queue length; the AC'97 status register exposes no separate FIFO-error
    /// counter here.
    pub underruns: u32,
    pub capacity_frames: u32,
    pub sample_rate: u32,
}

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
    /// The discovered AC'97 controller, or `None` when no card answered.
    ///
    /// WHY it is `Option` and not a dummy device: with no card every PCM entry
    /// point must return `Unavailable`. A placeholder that reported "stopped"
    /// would be indistinguishable from a real controller that had not been
    /// started yet.
    pub ac97: Option<Ac97Controller<PortAc97Io>>,
    /// The single bounded PCM output stream, when one is open.
    pub pcm_stream: Option<PcmStream>,
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
            ac97: None,
            pcm_stream: None,
        }
    }

    /// Whether a real AC'97 controller was discovered and initialized.
    pub fn pcm_device_present(&self) -> bool {
        self.ac97.as_ref().is_some_and(|driver| driver.is_ready())
    }

    /// Opens (or replaces) the single PCM output stream.
    ///
    /// Only the hardware's native format is accepted: the driver programs the
    /// codec for 48 kHz once and does not resample.
    pub fn open_pcm_stream(&mut self, format: PcmFormat) -> Result<(), AudioStreamError> {
        let driver = self.ac97.as_mut().ok_or(AudioStreamError::Unavailable)?;
        if !driver.is_ready() {
            return Err(AudioStreamError::Unavailable);
        }
        if format.sample_rate != PcmSampleRate::Hz48000
            || format.channels != PcmChannels::Stereo
            || format.bits_per_sample != 16
        {
            return Err(AudioStreamError::InvalidFormat);
        }
        // Replacing a stream must not leave the previous batch playing.
        driver.stop();
        let stream = PcmStream::new(format, PCM_STREAM_DEFAULT_FRAMES)
            .map_err(|_| AudioStreamError::InvalidFormat)?;
        self.pcm_stream = Some(stream);
        Ok(())
    }

    /// Enqueues PCM bytes and hands whatever the controller can take to DMA.
    ///
    /// Returns the bytes accepted into the bounded ring, which may be fewer
    /// than requested. `Ok(0)` is not produced: a caller that cannot enqueue
    /// anything is told the ring is full.
    pub fn write_pcm_stream(&mut self, bytes: &[u8]) -> Result<usize, AudioStreamError> {
        let driver = self.ac97.as_mut().ok_or(AudioStreamError::Unavailable)?;
        let stream = self.pcm_stream.as_mut().ok_or(AudioStreamError::NotOpen)?;
        let written = stream.write_bytes(bytes).map_err(|error| match error {
            PcmError::Full => AudioStreamError::Full,
            PcmError::InvalidLength | PcmError::InvalidGeometry | PcmError::Empty => {
                AudioStreamError::InvalidFormat
            }
        })?;
        driver.pump(stream).map_err(|_| AudioStreamError::DeviceFault)?;
        Ok(written)
    }

    /// Polls the controller, records an underrun when it went idle with nothing
    /// queued, and resumes a finished batch when data is waiting.
    pub fn poll_pcm_stream(&mut self) -> Result<AudioStreamStatus, AudioStreamError> {
        let driver = self.ac97.as_mut().ok_or(AudioStreamError::Unavailable)?;
        let device = driver.poll();
        if let Some(stream) = self.pcm_stream.as_mut() {
            if device.just_idle {
                if stream.queued_frames() == 0 {
                    stream.record_underrun();
                } else {
                    let _ = driver.pump(stream);
                }
            }
        }
        // A resumed batch is only visible in a fresh read of the run bit.
        let current = driver.status();
        Ok(self.pcm_status_from(Ac97Status {
            present: current.present,
            running: current.running || device.running,
            frames_submitted: current.frames_submitted,
            frames_completed: current.frames_completed,
            just_idle: device.just_idle,
        }))
    }

    fn pcm_status_from(&self, device: Ac97Status) -> AudioStreamStatus {
        let stream = self.pcm_stream.as_ref();
        AudioStreamStatus {
            device_present: device.present,
            device_running: device.running,
            stream_open: stream.is_some(),
            frames_written: stream.map_or(0, |s| s.frames_written()),
            frames_consumed: stream.map_or(0, |s| s.frames_consumed()),
            queued_frames: stream.map_or(0, |s| s.queued_frames() as u64),
            underruns: stream.map_or(0, |s| s.underruns()),
            capacity_frames: stream.map_or(0, |s| s.capacity_frames() as u32),
            sample_rate: stream.map_or(AC97_SAMPLE_RATE_HZ, |s| s.format().sample_rate as u32),
        }
    }

    /// Stops the controller and drops the stream. Safe when nothing is open.
    pub fn close_pcm_stream(&mut self) {
        if let Some(driver) = self.ac97.as_mut() {
            driver.stop();
        }
        self.pcm_stream = None;
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
        let level = if let Some(ref mut ac97) = self.ac97 {
            ac97.set_master_volume(stored)
        } else {
            stored
        };
        self.volume_percent = Some(level);
        level
    }

    /// Queries the real master volume from the AC'97 hardware mixer when present.
    pub fn get_volume(&mut self) -> Option<u8> {
        if let Some(ref mut ac97) = self.ac97 {
            let level = ac97.get_master_volume();
            self.volume_percent = Some(level);
            Some(level)
        } else {
            self.volume_percent
        }
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

    // 4. Bind a real AC'97 PCM controller. Absence is reported as absence: the
    //    stream syscalls return Unavailable rather than pretending to play.
    match Ac97Controller::probe() {
        Ok(mut driver) => {
            serial_println!(
                "[OK] AC'97 PCM output initialized: PCI {:04x}:{:04x} at {:02x}:{:02x}.{} (16-bit stereo, {} Hz); DMA buffers bound.",
                driver.vendor_id,
                driver.device_id,
                driver.pci_address.bus,
                driver.pci_address.device,
                driver.pci_address.function,
                AC97_SAMPLE_RATE_HZ,
            );
            let hw_volume = driver.get_master_volume();
            let mut mgr = AUDIO_MANAGER.lock();
            mgr.ac97 = Some(driver);
            mgr.volume_percent = Some(hw_volume);
        }
        Err(Ac97Error::Unavailable) => {
            serial_println!(
                "[AUDIO] PCM output unavailable: no AC'97 controller found on the PCI bus. \
                 AudioStreamOpen will return Unavailable."
            );
        }
        Err(error) => {
            serial_println!(
                "[AUDIO] PCM output unavailable: AC'97 initialization failed ({:?}). \
                 AudioStreamOpen will return Unavailable.",
                error
            );
        }
    }

    // 5. Test AudioManager Dispatch
    AUDIO_MANAGER.lock().play(AudioFeedback::Silence);
    serial_println!(
        "--- [Mobile Audio Subsystem: PIT speaker exercised; AC'97 PCM availability reported above] ---"
    );
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
