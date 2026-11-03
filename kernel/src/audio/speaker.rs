// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bare-Metal PC Speaker & PIT Channel 2 Hardware Tone Generator
//!
//! Directly drives x86 Programmable Interval Timer (PIT) Channel 2 (I/O Ports 0x42, 0x43)
//! and System Control Port B (I/O Port 0x61) to produce tactile touch haptic clicks,
//! notification chimes, and system alert audio.

use x86_64::instructions::port::Port;

const PIT_BASE_FREQUENCY: u32 = 1_193_182;
const PIT_CHANNEL2_DATA: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const SYSTEM_CONTROL_PORT_B: u16 = 0x61;

/// PIT Mode 3 (Square Wave Generator) for Channel 2 in 16-bit binary mode:
/// Bits 7-6: Channel 2 (10b)
/// Bits 5-4: Access mode lobyte/hibyte (11b)
/// Bits 3-1: Mode 3 (011b)
/// Bit 0: 16-bit binary (0b) -> 0xB6
const PIT_MODE3_CHANNEL2: u8 = 0xB6;

/// Hardware PC Speaker driver for mobile audible and haptic feedback.
pub struct Speaker;

impl Speaker {
    /// Computes the 16-bit PIT divisor for a target frequency in Hertz.
    #[inline]
    pub const fn compute_divisor(freq_hz: u32) -> u16 {
        if freq_hz == 0 {
            0
        } else {
            let div = PIT_BASE_FREQUENCY / freq_hz;
            if div > 65535 {
                65535
            } else if div < 1 {
                1
            } else {
                div as u16
            }
        }
    }

    /// Sets the frequency of PIT Channel 2.
    pub fn set_frequency(freq_hz: u32) {
        if freq_hz == 0 {
            Self::disable();
            return;
        }

        let divisor = Self::compute_divisor(freq_hz);

        unsafe {
            let mut cmd_port = Port::<u8>::new(PIT_COMMAND);
            cmd_port.write(PIT_MODE3_CHANNEL2);

            let mut data_port = Port::<u8>::new(PIT_CHANNEL2_DATA);
            data_port.write((divisor & 0xFF) as u8);
            data_port.write(((divisor >> 8) & 0xFF) as u8);
        }
    }

    /// Enables the speaker gate and output drivers on Port 0x61.
    pub fn enable() {
        unsafe {
            let mut ctrl_port = Port::<u8>::new(SYSTEM_CONTROL_PORT_B);
            let val = ctrl_port.read();
            if (val & 0x03) != 0x03 {
                ctrl_port.write(val | 0x03);
            }
        }
    }

    /// Disables the speaker output on Port 0x61.
    pub fn disable() {
        unsafe {
            let mut ctrl_port = Port::<u8>::new(SYSTEM_CONTROL_PORT_B);
            let val = ctrl_port.read();
            if (val & 0x03) != 0 {
                ctrl_port.write(val & !0x03);
            }
        }
    }

    /// Plays a continuous frequency tone.
    pub fn play_tone(freq_hz: u32) {
        Self::set_frequency(freq_hz);
        Self::enable();
    }

    /// Emits a tactile haptic click tone (1200 Hz micro-burst) for capacitive touch events.
    pub fn haptic_click() {
        Self::play_tone(1200);
        // Micro-delay loop in bare metal
        for _ in 0..40_000 {
            core::hint::spin_loop();
        }
        Self::disable();
    }

    /// Emits a smartphone notification chime (C5 523Hz -> E5 659Hz -> G5 784Hz).
    pub fn notification_chime() {
        let notes = [523, 659, 784];
        for &note in &notes {
            Self::play_tone(note);
            for _ in 0..120_000 {
                core::hint::spin_loop();
            }
        }
        Self::disable();
    }

    /// Emits a task confirmation jingle (D5 587Hz -> A5 880Hz).
    pub fn operation_confirmed_jingle() {
        let notes = [587, 880];
        for &note in &notes {
            Self::play_tone(note);
            for _ in 0..150_000 {
                core::hint::spin_loop();
            }
        }
        Self::disable();
    }

    /// Backwards compatibility alias for operation_confirmed_jingle.
    pub fn order_confirmed_jingle() {
        Self::operation_confirmed_jingle();
    }

    /// Emits a two-tone system alert chirp (880Hz -> 440Hz).
    pub fn alert_chirp() {
        let notes = [880, 440];
        for &note in &notes {
            Self::play_tone(note);
            for _ in 0..100_000 {
                core::hint::spin_loop();
            }
        }
        Self::disable();
    }
}
