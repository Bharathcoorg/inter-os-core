//! Virtual Device Filesystem (`/dev/`) Character Device Subsystem
//!
//! Exposes standard Unix-like character devices (`/dev/null`, `/dev/zero`,
//! `/dev/urandom`, `/dev/input/event0`, `/dev/ptmx`) to unprivileged tasks
//! through the microkernel VFS without ambient authority.

use crate::crypto::rng::get_random_bytes;
use crate::fs::FsError;
use crate::input::{INPUT_QUEUE, InputEvent};

/// Categorization of virtual device nodes supported by the microkernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Null,
    Zero,
    Urandom,
    InputEvent0,
    Ptmx,
}

/// Resolves a canonical VFS path to a supported character device kind.
pub fn resolve_device(path: &str) -> Option<DeviceKind> {
    match path {
        "/dev/null" => Some(DeviceKind::Null),
        "/dev/zero" => Some(DeviceKind::Zero),
        "/dev/urandom" => Some(DeviceKind::Urandom),
        "/dev/input/event0" => Some(DeviceKind::InputEvent0),
        "/dev/ptmx" => Some(DeviceKind::Ptmx),
        _ => None,
    }
}

/// Reads bytes from the specified virtual character device node.
pub fn device_read(path: &str, buf: &mut [u8]) -> Result<usize, FsError> {
    let kind = resolve_device(path).ok_or(FsError::FileNotFound)?;

    match kind {
        DeviceKind::Null => {
            // EOF on /dev/null
            Ok(0)
        }
        DeviceKind::Zero => {
            // Fill requested buffer with zeroes
            buf.fill(0);
            Ok(buf.len())
        }
        DeviceKind::Urandom => {
            // Fill requested buffer with hardware TRNG entropy
            get_random_bytes(buf);
            Ok(buf.len())
        }
        DeviceKind::InputEvent0 => {
            // Dequeue next hardware input event from the ring buffer
            let mut q = INPUT_QUEUE.lock();
            if let Some(event) = q.pop() {
                // Pack into a 16-byte fixed binary event record:
                // [0]: event_type (1: Key, 2: Touch, 3: Gesture)
                // [1]: slot / pressed
                // [2..4]: x (u16 le)
                // [4..6]: y (u16 le)
                // [6..8]: pressure (u16 le)
                // [8]: phase
                // [9..16]: reserved
                let mut record = [0u8; 16];
                match event {
                    InputEvent::Key { char_code, pressed } => {
                        record[0] = 1;
                        record[1] = if pressed { 1 } else { 0 };
                        record[2] = (char_code as u32 & 0xFF) as u8;
                    }
                    InputEvent::Touch(pt) => {
                        record[0] = 2;
                        record[1] = pt.slot;
                        let x_bytes = (pt.x as u16).to_le_bytes();
                        let y_bytes = (pt.y as u16).to_le_bytes();
                        let p_bytes = pt.pressure.to_le_bytes();
                        record[2..4].copy_from_slice(&x_bytes);
                        record[4..6].copy_from_slice(&y_bytes);
                        record[6..8].copy_from_slice(&p_bytes);
                        record[8] = pt.phase as u8;
                    }
                    InputEvent::Gesture(g) => {
                        record[0] = 3;
                        match g {
                            crate::input::TouchGesture::Tap { x, y } => {
                                record[1] = 1; // Tap
                                record[2..4].copy_from_slice(&(x as u16).to_le_bytes());
                                record[4..6].copy_from_slice(&(y as u16).to_le_bytes());
                            }
                            crate::input::TouchGesture::Drag { start_x, start_y, current_x, current_y } => {
                                record[1] = 2; // Drag
                                record[2..4].copy_from_slice(&(start_x as u16).to_le_bytes());
                                record[4..6].copy_from_slice(&(start_y as u16).to_le_bytes());
                                record[6..8].copy_from_slice(&(current_x as u16).to_le_bytes());
                                record[8..10].copy_from_slice(&(current_y as u16).to_le_bytes());
                            }
                            crate::input::TouchGesture::Release { x, y } => {
                                record[1] = 3; // Release
                                record[2..4].copy_from_slice(&(x as u16).to_le_bytes());
                                record[4..6].copy_from_slice(&(y as u16).to_le_bytes());
                            }
                        }
                    }
                }
                let copy_len = core::cmp::min(record.len(), buf.len());
                buf[..copy_len].copy_from_slice(&record[..copy_len]);
                Ok(copy_len)
            } else {
                // Non-blocking read returns 0 if queue is empty
                Ok(0)
            }
        }
        DeviceKind::Ptmx => {
            // Read returns 0 when no virtual console input is pending
            Ok(0)
        }
    }
}

/// Writes bytes to the specified virtual character device node.
pub fn device_write(path: &str, buf: &[u8]) -> Result<usize, FsError> {
    let kind = resolve_device(path).ok_or(FsError::FileNotFound)?;

    match kind {
        DeviceKind::Null | DeviceKind::Zero => {
            // Discard writes instantly
            Ok(buf.len())
        }
        DeviceKind::Urandom => {
            // Stir written entropy into hardware TRNG pool
            let mut val = 0u64;
            for (i, &b) in buf.iter().take(8).enumerate() {
                val |= (b as u64) << (i * 8);
            }
            crate::crypto::TRNG.lock().mix_entropy(val);
            Ok(buf.len())
        }
        DeviceKind::InputEvent0 => {
            // Input device node is read-only
            Err(FsError::PermissionDenied)
        }
        DeviceKind::Ptmx => {
            // Transmit bytes to COM1 serial / debug console
            for &b in buf {
                crate::serial_print!("{}", b as char);
            }
            Ok(buf.len())
        }
    }
}
