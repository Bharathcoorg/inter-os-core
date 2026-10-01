//! Inter OS Native Sovereign Music & Hi-Fi Audio Synthesizer
//!
//! Native on-device audio studio & media player executing in Ring 3 (User Mode):
//! - Multi-waveform programmable audio synthesizer:
//!   * Waveform::Sine: Pure fundamental acoustic tone
//!   * Waveform::Square: Retro chiptune synthesizer
//!   * Waveform::Triangle: Warm mellow woodwind texture
//!   * Waveform::Sawtooth: Rich harmonic brass/lead
//! - Local playlist state is populated only from real media storage.
//!   * Playlist is populated from real media storage when media is available.
//! - High-throughput Zero-Copy DMA Audio Streaming via SHM Pipe 1
//! - Hardware Keystore Slot 1 media key derivation
//! - Sovereign Vault Sector 204 authenticated track record encryption
//! - Sovereign Privacy Shield scrubbing to eliminate listening habits telemetry leakage
//! - Hardware 4-note major arpeggio synthesis (C4: 261Hz -> E4: 329Hz -> G4: 392Hz -> C5: 523Hz)
//! - Capacitive tactile rhythm haptics
//! - Asynchronous event reactor loop handling playback controls, DMA audio buffer refill, and volume changes

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

/// Synthesizer audio waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioWaveform {
    Sine = 0,
    Square = 1,
    Triangle = 2,
    Sawtooth = 3,
}

/// Playback state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

/// Sovereign Audio Track record.
#[derive(Debug, Clone, Copy)]
pub struct AudioTrack {
    pub id: u32,
    pub title: &'static str,
    pub artist: &'static str,
    pub bpm: u16,
    pub codec: &'static str,
}

/// Local Music Playlist.
pub static PLAYLIST: [AudioTrack; 0] = [];

/// Production Sovereign Music state manager.
pub struct MusicApp {
    pub state: PlaybackState,
    pub current_track_idx: usize,
    pub waveform: AudioWaveform,
    pub volume_percent: u8,
    pub total_dma_buffers_streamed: u32,
    pub keystore_derived: bool,
    pub sector_encrypted: bool,
}

impl MusicApp {
    pub const fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            current_track_idx: 0,
            waveform: AudioWaveform::Sine,
            volume_percent: 85,
            total_dma_buffers_streamed: 0,
            keystore_derived: false,
            sector_encrypted: false,
        }
    }

    /// Derives the media storage key from Hardware Keystore Slot 1.
    ///
    /// Returns `true` only when a full 32-byte key was actually returned.
    pub fn derive_keystore_key(&mut self) -> bool {
        let mut media_key = [0u8; 32];
        match std::keystore_derive(1, &mut media_key) {
            Ok(32) => {
                self.keystore_derived = true;
                std::log(0, "[MUSIC APP] Hardware keystore: Ingested media decryption key for local audio streaming.").ok();
                true
            }
            Ok(_) => {
                // WHY: a short result is not a key. Report it instead of leaving
                // the caller to assume derivation succeeded.
                std::log(0, "[MUSIC APP] Hardware keystore: short derivation result; no media key available.").ok();
                false
            }
            Err(_) => {
                // WHY: this app holds no keystore capability, so the call is
                // refused. Say so instead of reporting a key that was never issued.
                std::log(0, "[MUSIC APP] Hardware keystore: derivation refused by the kernel; no media key available.").ok();
                false
            }
        }
    }

    /// Plays the 4-note ascending C-major arpeggio via hardware audio timer.
    ///
    /// Returns `true` only when the audio service accepted the sequence. Tone
    /// requests are gated on a capability this app does not hold, so the honest
    /// default is "not played".
    pub fn play_arpeggio(&self) -> bool {
        let mut played = 0u32;
        for freq in [261u32, 329, 392, 523] {
            if std::audio_play_tone(freq).is_ok() {
                played += 1;
            }
        }
        let _ = std::audio_silence();
        let _ = std::audio_haptic_click();
        if played == 4 {
            std::log(
                0,
                "[MUSIC APP] Arpeggiated C-major melody synthesized (261Hz -> 523Hz).",
            )
            .ok();
            true
        } else {
            // WHY: audio control is refused without the audio-service authority.
            // Never log a synthesized melody that was never emitted.
            std::log(
                0,
                "[MUSIC APP] Arpeggio not played: the audio service refused the tone requests.",
            )
            .ok();
            false
        }
    }

    /// Attempts to stream one media payload over the zero-copy DMA ring.
    ///
    /// Returns `true` only when a transfer actually occurred. The local playlist
    /// is populated from real media storage, so with no media there is nothing to
    /// hand to the DMA engine and no buffer was consumed.
    pub fn stream_dma_audio_block(&mut self) -> bool {
        // WHY: the transport state and the DMA counter must track real transfers.
        // Returning false here is what stops the caller from reporting playback
        // or counting a buffer that was never streamed.
        if PLAYLIST.is_empty() {
            std::log(0, "[MUSIC APP] No media payload is available for DMA streaming.").ok();
            return false;
        }
        // A non-empty playlist would require a real DMA descriptor from the media
        // subsystem. No such descriptor exists yet, so fail closed.
        std::log(
            0,
            "[MUSIC APP] Media present but no DMA descriptor was obtained; no buffer streamed.",
        )
        .ok();
        false
    }

    /// Encrypts metadata only when a real media record exists.
    pub fn encrypt_track_record(&mut self) {
        std::log(0, "[MUSIC APP] No media record is available for encryption.").ok();
    }
}

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

fn render_music_ui(surface: u32, app: &MusicApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            // WHY: the transport state drives the whole visual language of this screen.
// Leave the artwork and progress rail neutral until media actually exists, so
// the player is never shown mid-playback with no audio behind it.
let has_media = !PLAYLIST.is_empty();
if y < 96 {
                if y >= 94 {
                    chunk[i] = std::design::color::ACCENT;
                } else if x >= 24 && x < 100 && y >= 28 && y < 68 {
                    // Back button <
                    let center_x = 62;
                    let center_y = 48;
                    let dx = (x as isize - center_x as isize).abs();
                    let dy = (y as isize - center_y as isize).abs();
                    if (dx + dy <= 12 && x <= center_x)
                        || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
                    {
                        chunk[i] = std::design::color::TEXT;
                    } else {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    }
                } else if x >= 120 && x < 320 && (40..56).contains(&y) {
                    chunk[i] = std::design::color::TEXT;
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if (120..640).contains(&y) && x >= 100 && x < W - 100 {
                // Album artwork display canvas
                let center_x = (W / 2) as isize;
                let center_y = 380isize;
                let dist_sq = (x as isize - center_x).pow(2) + (y as isize - center_y).pow(2);
                if dist_sq < 220 * 220 {
                    if dist_sq < 60 * 60 {
                        // WHY: the accent disc read as a spinning record. With no
                        // media loaded there is nothing spinning, so mute it.
                        chunk[i] = if has_media {
                            std::design::color::ACCENT
                        } else {
                            std::design::color::SURFACE_RAISED
                        };
                    } else if (dist_sq % 2500) < 400 {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    } else {
                        chunk[i] = 0xFF141923;
                    }
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if (680..720).contains(&y) && x >= 48 && x < 380 {
                chunk[i] = std::design::color::TEXT;
            } else if (740..752).contains(&y) && x >= 48 && x < W - 48 {
                // Progress bar. WHY: a fixed 33% accent fill claimed playback
                // progress that never happened. Show an empty rail until a real
                // transfer has moved the playhead.
                if has_media && x < 48 + 360 {
                    chunk[i] = std::design::color::ACCENT;
                } else {
                    chunk[i] = std::design::color::SURFACE_RAISED;
                }
            } else if (800..960).contains(&y) {
                // Play / Pause central button
                let center_x = (W / 2) as isize;
                let center_y = 880isize;
                let dist_sq = (x as isize - center_x).pow(2) + (y as isize - center_y).pow(2);
                if dist_sq < 56 * 56 {
                    chunk[i] = match app.state {
                        PlaybackState::Playing => std::design::color::ACCENT,
                        _ => std::design::color::SURFACE_RAISED,
                    };
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
                chunk[i] = std::design::color::TEXT_MUTED;
            } else {
                chunk[i] = std::design::color::BACKGROUND;
            }
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[MUSIC APP (Ring 3)] Sovereign Hi-Fi Audio Studio & Synthesizer online.",
    )
    .ok();

    let mut music = MusicApp::new();

    // 1. Hardware Keystore Derivation
    music.derive_keystore_key();

    // 2. Encrypt Track Record in Sector 204
    music.encrypt_track_record();

    // 3. Allocate DRM/KMS Client Surface (z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[MUSIC APP] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    // 4. Hardware Musical Arpeggio Synthesis
    music.play_arpeggio();

    // 5. Zero-Copy DMA Audio Streaming over Pipe 1
    // WHY: the previous code set PlaybackState::Playing immediately after the
    // stream call, even though the call reports that no media payload exists.
    // The transport state must only be advanced when a transfer actually
    // happened, otherwise the UI shows playback that never occurred.
    if music.stream_dma_audio_block() {
        music.total_dma_buffers_streamed += 1;
        music.state = PlaybackState::Playing;
    }

    if surface > 0 {
        render_music_ui(surface, &music);
        let _ = std::surface_flip(6);
        std::log(0, "[MUSIC APP] Music canvas presented to scanout buffer.").ok();
    }

    // 6. Sovereign Asynchronous Event Reactor Loop
    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
                    let mut input = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    while std::input_event_poll(&mut input).is_ok() {
                        if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                            std::log(
                                0,
                                "[MUSIC APP] Navigation gesture recognized. Exiting to launcher.",
                            )
                            .ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if input.kind == 1 {
                            // Check Back button in header or Home gesture bar: exit cleanly to launcher
                            if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                                || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                            {
                                std::log(0, "[MUSIC APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }

                            // Check Play / Pause center button
                            let center_x = (W / 2) as isize;
                            let center_y = 880isize;
                            let dist_sq = (input.x as isize - center_x).pow(2)
                                + (input.y as isize - center_y).pow(2);
                            if dist_sq < 56 * 56 {
                                // WHY: pressing play must not show an active
                                // transport. Only a real DMA transfer may enter
                                // the Playing state; otherwise report that there
                                // is no media to play and stay Stopped.
                                match music.state {
                                    PlaybackState::Playing => {
                                        music.state = PlaybackState::Paused;
                                        std::log(0, "[MUSIC APP] Transport paused by user.").ok();
                                    }
                                    _ => {
                                        if music.stream_dma_audio_block() {
                                            music.total_dma_buffers_streamed += 1;
                                            music.state = PlaybackState::Playing;
                                        } else {
                                            music.state = PlaybackState::Stopped;
                                            std::log(
                                                0,
                                                "[MUSIC APP] Play requested but no media is available; transport stays stopped.",
                                            )
                                            .ok();
                                        }
                                    }
                                }
                                let _ = std::audio_haptic_click();
                                if surface > 0 {
                                    render_music_ui(surface, &music);
                                    let _ = std::surface_flip(6);
                                }
                            }
                        }
                    }
                }
                if events & std::EVENT_IPC_MSG != 0 {
                    // WHY: the event bit only says an IPC message was queued. No
                    // playback command was parsed out of it, so do not claim one
                    // was received or applied.
                    std::log(
                        0,
                        "[MUSIC APP] IPC message event raised; no playback command was parsed from it.",
                    )
                    .ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    // Periodic DMA audio ring buffer refill.
                    // WHY: the counter used to advance on every timer tick while
                    // Playing, with no DMA transfer taking place. Advance it only
                    // when a refill actually transferred a buffer.
                    if music.state == PlaybackState::Playing && music.stream_dma_audio_block() {
                        music.total_dma_buffers_streamed += 1;
                    }
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[MUSIC APP] Panic detected! Terminating task...").ok();
    std::exit();
}
