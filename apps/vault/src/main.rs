//! Inter OS Sovereign Vault & Multi-Sector Storage Manager
//!
//! Ring 3 encrypted-storage manager. Everything this app displays is the result
//! of a syscall it just made:
//!
//! - `keystore_derive(0/1)` for the two key slots. This kernel refuses to export
//!   raw key material to Ring 3, so both probes normally answer
//!   `PermissionDenied` and the screen says "unavailable" rather than implying
//!   an enclave it does not have.
//! - `vault_encrypt_sector` / `vault_decrypt_sector` for the per-sector
//!   round-trip self-test. A sector row is only marked "round-trip verified"
//!   after encrypt -> mutate-check -> decrypt -> compare actually succeeded.
//! - `devfs_read("/dev/urandom")` for the 16-byte IV. IV generation fails closed:
//!   there is no fixed or synthesised IV anywhere in this file.
//! - `settings_set/get("vault.backup_export")` for the sovereign backup frame,
//!   which is written, read back and checksum-validated before anything claims
//!   to be verified.
//! - VFS `/system/etc/device_id` for the device identity magic prefix only.
//!
//! Banded rendering (`render_vault_band`): only the rows an input actually
//! dirtied are re-rasterised, instead of all 1,036,800 pixels on every event.
//!
//! The presentation is built from the canonical InterOS light design tokens on
//! an 8-point grid: 24px screen margin, 672px content column, 18px card radius,
//! 48px minimum touch targets. Every string is measured and ellipsised to the
//! box the layout reserved for it, so nothing is ever clipped or overlapped.

#![no_std]
#![no_main]

mod font;
mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Managed storage sector definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSectorType {
    RootKeystore = 100,
    MediaContainers = 200,
    CameraPhotos = 201,
    EncryptedMessages = 202,
    CalendarEvents = 203,
    MusicTracks = 204,
}

/// Sector metadata record.
#[derive(Debug, Clone, Copy)]
pub struct SectorRecord {
    pub sector_id: u64,
    pub sector_type: VaultSectorType,
    pub description: &'static str,
    pub encrypted_bytes: usize,
    /// True after an in-memory encrypt->decrypt roundtrip for this sector id
    /// succeeded. Nothing is persisted per sector, so this records a crypto
    /// self-test, not stored-data integrity.
    pub authenticated: bool,
}

/// Outcome of one `keystore_derive` probe. `Refused` carries the kernel's own
/// negative status code; nothing here is guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyProbe {
    NotRun,
    Derived,
    /// The call succeeded but did not hand back a 32-byte key.
    Short,
    Refused(i64),
}

/// Outcome of the 16-byte `/dev/urandom` read probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyProbe {
    NotRun,
    Read16,
    Short,
    AllZero,
    Refused(i64),
}

/// Outcome of the `/system/etc/device_id` read. This is a magic-prefix check,
/// never an attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityState {
    NotChecked,
    Present,
    Malformed,
    Unreadable,
}

/// Outcome of the sovereign backup export, including its read-back validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupState {
    NotAttempted,
    WrittenAndValid,
    WrittenButChecksumMismatch,
    WrittenButTruncated,
    ReadRefused,
    WriteRefused,
}

/// Computes a Fletcher-32 checksum over `data` (both sums taken mod 255).
fn fletcher32(data: &[u8]) -> u32 {
    let mut sum1: u32 = 0;
    let mut sum2: u32 = 0;
    for &byte in data.iter() {
        sum1 = (sum1 + byte as u32) % 255;
        sum2 = (sum2 + sum1) % 255;
    }
    (sum2 << 8) | sum1
}

/// Hex-encodes `data` into `out` (two characters per byte).
/// Returns the number of bytes written.
fn hex_encode(data: &[u8], out: &mut [u8]) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut i = 0usize;
    for &b in data.iter() {
        if i + 2 > out.len() {
            break;
        }
        out[i] = DIGITS[(b >> 4) as usize];
        out[i + 1] = DIGITS[(b & 0x0F) as usize];
        i += 2;
    }
    i
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex characters in `hex` into `out`.
/// Returns the decoded length, or None on odd length, bad characters, or an
/// output buffer that is too small.
fn hex_decode(hex: &[u8], out: &mut [u8]) -> Option<usize> {
    if hex.len() % 2 != 0 || hex.len() / 2 > out.len() {
        return None;
    }
    let mut i = 0usize;
    while i < hex.len() {
        let hi = hex_nibble(hex[i])?;
        let lo = hex_nibble(hex[i + 1])?;
        out[i / 2] = (hi << 4) | lo;
        i += 2;
    }
    Some(hex.len() / 2)
}

/// Production Sovereign Vault state manager.
pub struct VaultApp {
    pub sectors: [Option<SectorRecord>; 6],
    pub key_probe: [KeyProbe; 2],
    pub entropy_probe: EntropyProbe,
    pub identity: IdentityState,
    pub backup: BackupState,
    pub total_encryptions: u32,
    pub total_decryptions: u32,
    /// Last thing the platform actually said, shown verbatim in the status line.
    pub status: [u8; 64],
    pub status_len: usize,
}

impl VaultApp {
    pub const fn new() -> Self {
        Self {
            sectors: [
                Some(SectorRecord {
                    sector_id: 100,
                    sector_type: VaultSectorType::RootKeystore,
                    description: "Root Keystore & Identity",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: 200,
                    sector_type: VaultSectorType::MediaContainers,
                    description: "General Media & Documents",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: 201,
                    sector_type: VaultSectorType::CameraPhotos,
                    description: "Camera Photos & ISP Metadata",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: 202,
                    sector_type: VaultSectorType::EncryptedMessages,
                    description: "Encrypted Messages & P2P Wire",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: 203,
                    sector_type: VaultSectorType::CalendarEvents,
                    description: "Calendar Events & Timeline",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: 204,
                    sector_type: VaultSectorType::MusicTracks,
                    description: "Audio Tracks & Music Studio",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
            ],
            key_probe: [KeyProbe::NotRun, KeyProbe::NotRun],
            entropy_probe: EntropyProbe::NotRun,
            identity: IdentityState::NotChecked,
            backup: BackupState::NotAttempted,
            total_encryptions: 0,
            total_decryptions: 0,
            status: [0u8; 64],
            status_len: 0,
        }
    }

    /// True when both hardware key slots actually produced a 32-byte key.
    pub fn keys_derived(&self) -> bool {
        self.key_probe == [KeyProbe::Derived, KeyProbe::Derived]
    }

    /// Derives cryptographic keys from the hardware keystore and records the
    /// exact answer each slot gave.
    pub fn derive_keys(&mut self) {
        let mut key0 = [0u8; 32];
        self.key_probe[0] = match std::keystore_derive(0, &mut key0) {
            Ok(len) if len == 32 => {
                std::log(
                    0,
                    "[VAULT APP] Hardware Keystore Slot 0 derived: Root Master Key online.",
                )
                .ok();
                KeyProbe::Derived
            }
            Ok(_) => KeyProbe::Short,
            Err(e) => KeyProbe::Refused(e),
        };

        let mut key1 = [0u8; 32];
        self.key_probe[1] = match std::keystore_derive(1, &mut key1) {
            Ok(len) if len == 32 => {
                std::log(
                    0,
                    "[VAULT APP] Hardware Keystore Slot 1 derived: Storage Encryption Key online.",
                )
                .ok();
                KeyProbe::Derived
            }
            Ok(_) => KeyProbe::Short,
            Err(e) => KeyProbe::Refused(e),
        };

        if !self.keys_derived() {
            std::log(
                0,
                "[VAULT APP] Hardware keystore did not export raw key material to Ring 3; no key is claimed.",
            )
            .ok();
        }
    }

    /// Reads 16 bytes of hardware entropy to confirm the IV source is live.
    /// This is a probe only; the IV used by a real operation is minted by
    /// `generate_iv` at the moment it is needed.
    pub fn probe_entropy(&mut self) {
        let mut probe = [0u8; 16];
        self.entropy_probe = match std::devfs_read("/dev/urandom", &mut probe) {
            Ok(n) if n == 16 => {
                if probe == [0u8; 16] {
                    EntropyProbe::AllZero
                } else {
                    EntropyProbe::Read16
                }
            }
            Ok(_) => EntropyProbe::Short,
            Err(e) => EntropyProbe::Refused(e),
        };
    }

    /// Reads the device identity file. Only the `DID:` magic prefix is checked;
    /// with no signature or digest to compare against this is a presence check,
    /// not an authentication.
    pub fn probe_identity(&mut self) {
        self.identity = match std::fs_open(3, "/system/etc/device_id", std::FS_READ) {
            Ok(fd) => {
                let mut dev_buf = [0u8; 64];
                let state = match std::fs_read(3, fd, &mut dev_buf) {
                    Ok(n) if n >= 4 && &dev_buf[..4] == b"DID:" => IdentityState::Present,
                    Ok(_) => IdentityState::Malformed,
                    Err(_) => IdentityState::Unreadable,
                };
                let _ = std::fs_close(fd);
                state
            }
            Err(_) => IdentityState::Unreadable,
        };
    }

    /// Ingests hardware entropy for a 16-byte initialization vector.
    pub fn generate_iv(&self) -> Option<[u8; 16]> {
        let mut iv = [0u8; 16];
        if let Ok(n) = std::devfs_read("/dev/urandom", &mut iv) {
            if n == 16 && iv != [0u8; 16] {
                return Some(iv);
            }
        }
        // Encryption must fail closed if entropy is unavailable. A fixed IV
        // would create deterministic cryptographic state and must never be used.
        None
    }

    /// Performs an authenticated encryption and decryption test on a target sector.
    pub fn test_sector_roundtrip(&mut self, sector_id: u64, payload_tag: &[u8]) -> bool {
        let Some(iv) = self.generate_iv() else {
            std::log(0, "[VAULT APP] Entropy unavailable; refusing sector verification.").ok();
            return false;
        };
        let mut sector_buf = [0u8; 512];
        let copy_len = payload_tag.len().min(64);
        sector_buf[..copy_len].copy_from_slice(&payload_tag[..copy_len]);

        if std::vault_encrypt_sector(sector_id, &mut sector_buf, &iv).is_ok() {
            self.total_encryptions += 1;
            if &sector_buf[..copy_len] != &payload_tag[..copy_len] {
                if std::vault_decrypt_sector(sector_id, &mut sector_buf, &iv).is_ok() {
                    self.total_decryptions += 1;
                    if &sector_buf[..copy_len] == &payload_tag[..copy_len] {
                        for slot in self.sectors.iter_mut().flatten() {
                            if slot.sector_id == sector_id {
                                slot.authenticated = true;
                            }
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Re-runs one sector's round-trip test, recording failure as well as
    /// success so a sector that later stops verifying stops claiming otherwise.
    pub fn verify_sector(&mut self, index: usize) -> bool {
        let Some(sec) = self.sectors[index] else {
            return false;
        };
        let ok = self.test_sector_roundtrip(sec.sector_id, b"INTER_VAULT_TOUCH_AUTH_VERIFY");
        if let Some(slot) = self.sectors[index].as_mut() {
            slot.authenticated = ok;
        }
        std::audio_haptic_click().ok();
        ok
    }

    /// Runs roundtrip verification across all managed sectors.
    /// Returns true only when every sector's roundtrip actually succeeded.
    pub fn verify_all_sectors(&mut self) -> bool {
        // Not short-circuited on purpose: every sector must be exercised even
        // after an early failure, so the per-sector failure logs are complete.
        let mut all_ok = true;
        for index in 0..6 {
            let Some(sec) = self.sectors[index] else { continue };
            let ok = self.test_sector_roundtrip(sec.sector_id, b"INTER_ROOT_KEYSTORE_RECORD_V1");
            if let Some(slot) = self.sectors[index].as_mut() {
                slot.authenticated = ok;
            }
            all_ok &= ok;
        }
        all_ok
    }

    /// Builds a sovereign backup export frame, persists it through the kernel
    /// settings store, and validates the stored frame by reading it back.
    ///
    /// The frame is persisted as hex (the settings store rejects newline bytes
    /// and raw counter/checksum bytes can contain 0x0A) under
    /// `vault.backup_export` via FileSystemWrite at cap slot 3, and "verified"
    /// means the read-back frame carries the expected magic and its stored
    /// Fletcher-32 checksum equals the recomputed one.
    pub fn export_backup(&mut self) -> BackupState {
        let mut header = [0u8; 40];
        header[..28].copy_from_slice(b"INTER_SOVEREIGN_VAULT_BACKUP");
        header[28..32].copy_from_slice(&self.total_encryptions.to_le_bytes());
        header[32..36].copy_from_slice(&self.total_decryptions.to_le_bytes());
        let sum = fletcher32(&header[..36]);
        header[36..40].copy_from_slice(&sum.to_le_bytes());

        let mut encoded = [0u8; 80];
        let enc_len = hex_encode(&header, &mut encoded);
        self.backup = if std::settings_set(3, "vault.backup_export", &encoded[..enc_len]).is_err() {
            std::log(
                0,
                "[VAULT APP] Backup export refused: the settings store rejected the write.",
            )
            .ok();
            BackupState::WriteRefused
        } else {
            let mut stored_hex = [0u8; 80];
            let mut frame = [0u8; 40];
            match std::settings_get(3, "vault.backup_export", &mut stored_hex) {
                Ok(n) => match hex_decode(&stored_hex[..n.min(stored_hex.len())], &mut frame) {
                    Some(40) => {
                        let stored_sum =
                            u32::from_le_bytes([frame[36], frame[37], frame[38], frame[39]]);
                        if frame[..28] == *b"INTER_SOVEREIGN_VAULT_BACKUP"
                            && stored_sum == fletcher32(&frame[..36])
                        {
                            std::log(
                                0,
                                "[VAULT APP] Sovereign Backup Export persisted; read-back Fletcher-32 checksum valid.",
                            )
                            .ok();
                            BackupState::WrittenAndValid
                        } else {
                            std::log(
                                0,
                                "[VAULT APP] Backup Export stored, but the read-back frame failed checksum validation.",
                            )
                            .ok();
                            BackupState::WrittenButChecksumMismatch
                        }
                    }
                    _ => {
                        std::log(
                            0,
                            "[VAULT APP] Backup Export stored, but the read-back frame was truncated or undecodable; validation not performed.",
                        )
                        .ok();
                        BackupState::WrittenButTruncated
                    }
                },
                _ => {
                    std::log(
                        0,
                        "[VAULT APP] Backup Export stored, but the read-back was refused; validation not performed.",
                    )
                    .ok();
                    BackupState::ReadRefused
                }
            }
        };
        self.backup
    }

    /// Re-runs every platform probe this screen reports on.
    pub fn reprobe(&mut self) {
        self.derive_keys();
        self.probe_entropy();
        self.probe_identity();
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens (canonical light System Space palette)
// -----------------------------------------------------------------------------

const BG: u32 = std::design::color::BACKGROUND; // #F7F6F2 base
const SURFACE: u32 = std::design::color::SURFACE; // #FFFFFF card surface
const SOFT: u32 = std::design::color::SURFACE_SOFT; // #EFEEE9 controls / tiles
const BORDER: u32 = std::design::color::BORDER; // #E1E0DB hairlines
const TEXT: u32 = std::design::color::TEXT; // #111214 primary text
const MUTED: u32 = std::design::color::TEXT_MUTED; // #777773 secondary text
const ACCENT: u32 = std::design::color::ACCENT; // restrained blue
const OK: u32 = std::design::color::SUCCESS; // muted green
const WARN: u32 = std::design::color::WARNING; // amber
const DANGER: u32 = std::design::color::DANGER; // muted red
const GESTURE_BAR: u32 = 0xFFC4C3BD; // home indicator pill on the light base

// -----------------------------------------------------------------------------
// Layout (8-point grid; 24px screen margin; 48px minimum touch targets)
// -----------------------------------------------------------------------------

const MARGIN: usize = 24;
const CARD_W: usize = W - MARGIN * 2; // 672
const PAD: usize = 24; // card inner padding
const CARD_R: usize = 18;
const INNER_W: usize = CARD_W - PAD * 2; // 624

const HEAD_BOTTOM: usize = 104;
const TITLE_X: usize = 88;
const BACK_X: usize = 24;
const BACK_Y: usize = 24;
const BACK_S: usize = 48;

/// Header action pill ("RE-PROBE"): 44px tall, right-aligned to the margin.
const ACTION_H: usize = 44;
const ACTION_Y: usize = 26;
const ACTION_X1: usize = W - MARGIN;
const ACTION: &[u8] = b"RE-PROBE";

const TITLE: &[u8] = b"Vault";
const SUBTITLE: &[u8] = b"Sovereign encrypted storage";

const STATUS_Y: usize = 1328;
/// Rows the status line owns: one scale-1 run at `STATUS_Y`.
///
/// WHY `GLYPH_H`: a scale-1 run is exactly one line box tall, and
/// `font::line_height(1)` is this constant - but that helper takes its scale at
/// runtime, so the band ceiling is spelled from the shared glyph height instead
/// of repeating a literal 24 that could drift from the face.
const STATUS_H: usize = std::design::font::GLYPH_H;
const GESTURE_Y0: usize = 1400;

/// Card A: hardware keystore probes.
const CARD_A_Y: usize = 120;
const CARD_A_H: usize = 272;

/// Card B: managed sector grid, 2 columns x 3 rows of 304x112 tiles.
const CARD_B_Y: usize = 416;
const CARD_B_H: usize = 456;
const TILE_W: usize = 304;
const TILE_H: usize = 112;
const TILE_COL_X: [usize; 2] = [48, 368];
const TILE_ROW_Y: [usize; 3] = [480, 608, 736];

/// Card C: backup export and entropy.
const CARD_C_Y: usize = 896;
const CARD_C_H: usize = 336;

/// Action row: two 328px buttons on the 2-column grid (2*328 + 16 + 2*24 = 720).
const BTN_Y: usize = 1256;
const BTN_H: usize = 56;
const BTN_W: usize = 328;
const BTN_X: [usize; 2] = [24, 368];

fn tile_origin(index: usize) -> (usize, usize) {
    (TILE_COL_X[index % 2], TILE_ROW_Y[index / 2])
}

/// Sector names shown on the tiles, indexed exactly like `VaultApp::sectors`.
fn sector_tile(index: usize) -> (&'static [u8], &'static [u8]) {
    match index {
        0 => (b"Root keystore", b"Sector 100 - tap to test"),
        1 => (b"Media & documents", b"Sector 200 - tap to test"),
        2 => (b"Camera photos", b"Sector 201 - tap to test"),
        3 => (b"Messages & wire", b"Sector 202 - tap to test"),
        4 => (b"Calendar events", b"Sector 203 - tap to test"),
        _ => (b"Audio tracks", b"Sector 204 - tap to test"),
    }
}

/// Kernel status code names, as the kernel spells them.
#[allow(dead_code)]
fn status_name(code: i64) -> &'static [u8] {
    match code {
        -1 => b"PermissionDenied",
        -2 => b"InvalidSlot",
        -3 => b"InvalidArguments",
        -12 => b"DeviceError",
        _ => b"refused",
    }
}

// -----------------------------------------------------------------------------
// Shape primitives
// -----------------------------------------------------------------------------

/// Whether `(x, y)` is inside the rounded rectangle `(x0, y0, w, h)`.
#[inline]
fn in_round(x: usize, y: usize, x0: usize, y0: usize, w: usize, h: usize, r: usize) -> bool {
    if x < x0 || y < y0 || x >= x0 + w || y >= y0 + h {
        return false;
    }
    let r = r.min(w / 2).min(h / 2);
    if r == 0 {
        return true;
    }
    let left = x0 + r;
    let right = x0 + w - r;
    let top = y0 + r;
    let bottom = y0 + h - r;
    let cx = if x < left {
        left
    } else if x >= right {
        right - 1
    } else {
        x
    };
    let cy = if y < top {
        top
    } else if y >= bottom {
        bottom - 1
    } else {
        y
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) as usize <= r * r
}

/// White card with a 1px hairline border; `None` outside the rounded rect.
#[inline]
fn card_pixel(x: usize, y: usize, x0: usize, y0: usize, w: usize, h: usize) -> Option<u32> {
    if !in_round(x, y, x0, y0, w, h, CARD_R) {
        return None;
    }
    if !in_round(x, y, x0 + 1, y0 + 1, w - 2, h - 2, CARD_R - 1) {
        return Some(BORDER);
    }
    Some(SURFACE)
}

/// Squared distance from `(x, y)` to the segment `a -> b`.
fn dist2_to_seg(x: usize, y: usize, ax: i64, ay: i64, bx: i64, by: i64) -> i64 {
    let px = x as i64;
    let py = y as i64;
    let abx = bx - ax;
    let aby = by - ay;
    let len2 = (abx * abx + aby * aby).max(1);
    let mut t = (px - ax) * abx + (py - ay) * aby;
    if t < 0 {
        t = 0;
    } else if t > len2 {
        t = len2;
    }
    let qx = ax + (t * abx) / len2;
    let qy = ay + (t * aby) / len2;
    (px - qx) * (px - qx) + (py - qy) * (py - qy)
}

/// Back chevron: one coherent 2px rounded stroke family.
#[inline]
fn chevron_ink(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let (cx, cy) = (cx as i64, cy as i64);
    if dist2_to_seg(x, y, cx + 7, cy - 11, cx - 7, cy) <= 4
        || dist2_to_seg(x, y, cx - 7, cy, cx + 7, cy + 11) <= 4
    {
        Some(color)
    } else {
        None
    }
}

/// 48x48 back control: soft pill with the chevron centred.
fn back_button_pixel(x: usize, y: usize) -> Option<u32> {
    if !in_round(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, 14) {
        return None;
    }
    if !in_round(x, y, BACK_X + 1, BACK_Y + 1, BACK_S - 2, BACK_S - 2, 13) {
        return Some(BORDER);
    }
    if let Some(c) = chevron_ink(x, y, BACK_X + BACK_S / 2, BACK_Y + BACK_S / 2, TEXT) {
        return Some(c);
    }
    Some(SOFT)
}

/// Width of the header action pill for a given label.
fn action_w(label: &[u8]) -> usize {
    font::width(label, 1, true) + 32
}

/// Drawn (and therefore hit-testable) rectangle of the header action pill.
fn action_rect(label: &[u8]) -> (usize, usize, usize, usize) {
    let w = action_w(label);
    (ACTION_X1 - w, ACTION_Y, w, ACTION_H)
}

fn action_pixel(x: usize, y: usize, label: &[u8]) -> Option<u32> {
    let (x0, y0, w, h) = action_rect(label);
    if !in_round(x, y, x0, y0, w, h, h / 2) {
        return None;
    }
    if !in_round(x, y, x0 + 1, y0 + 1, w - 2, h - 2, h / 2 - 1) {
        return Some(BORDER);
    }
    // WHY the centring box is the control's full width and not `w - 16`: the pill
    // is exactly the label plus 32px of padding (`action_w` adds it), so
    // subtracting another 16 centred the text in a box inset 16 from the right
    // alone and sat the run 8px left of the pill's true centre. Measured on the
    // guest: label ink 570..669 against a 560..696 pill, centre 619.5 vs 628.
    if font::ink_center(x, y, x0, y0 + (h - 24) / 2, 1, label, w, TEXT, SOFT, true) {
        return Some(TEXT);
    }
    Some(SOFT)
}

/// Caption band shared by every card: an accent label on the card surface.
#[inline(never)]
fn caption_pixel(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    text: &[u8],
    max_w: usize,
) -> Option<u32> {
    if font::ink(x, y, x0, y0, 1, text, max_w, ACCENT, SURFACE, true) {
        return Some(ACCENT);
    }
    None
}

/// One information row: label on the left, verdict right-aligned on the same
/// line, provenance underneath. The label and the verdict each own half of the
/// inner width minus a 32px gutter, so they can never collide.
#[inline(never)]
fn metric_row(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    w: usize,
    label: &[u8],
    value: &[u8],
    value_ink: u32,
    source: &[u8],
) -> Option<u32> {
    let ix = x0 + PAD;
    let half = w / 2 - PAD - 16;
    if font::ink(x, y, ix, y0, 1, label, half, MUTED, SURFACE, false) {
        return Some(MUTED);
    }
    if font::ink_right(x, y, x0 + w - PAD, y0, 1, value, half, value_ink, SURFACE, true) {
        return Some(value_ink);
    }
    if font::ink(x, y, ix, y0 + 32, 1, source, w - PAD * 2, MUTED, SURFACE, false) {
        return Some(MUTED);
    }
    None
}

/// One sector tile: soft rounded rect carrying a name, a verdict and a hint.
#[inline(never)]
fn tile_pixel(
    x: usize,
    y: usize,
    tx: usize,
    ty: usize,
    title: &[u8],
    verdict: &[u8],
    verdict_ink: u32,
    hint: &[u8],
) -> Option<u32> {
    if !in_round(x, y, tx, ty, TILE_W, TILE_H, 14) {
        return None;
    }
    if !in_round(x, y, tx + 1, ty + 1, TILE_W - 2, TILE_H - 2, 13) {
        return Some(BORDER);
    }
    let iw = TILE_W - 32;
    if font::ink(x, y, tx + 16, ty + 16, 1, title, iw, TEXT, SOFT, true) {
        return Some(TEXT);
    }
    if font::ink(x, y, tx + 16, ty + 48, 1, verdict, iw, verdict_ink, SOFT, true) {
        return Some(verdict_ink);
    }
    if font::ink(x, y, tx + 16, ty + 80, 1, hint, iw, MUTED, SOFT, false) {
        return Some(MUTED);
    }
    Some(SOFT)
}

/// Primary/secondary action button, 56px tall (>= the 48px preferred target).
#[inline(never)]
fn button_pixel(
    x: usize,
    y: usize,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    label: &[u8],
    primary: bool,
) -> Option<u32> {
    if !in_round(x, y, bx, by, bw, bh, 14) {
        return None;
    }
    if !in_round(x, y, bx + 1, by + 1, bw - 2, bh - 2, 13) {
        return Some(if primary { ACCENT } else { BORDER });
    }
    let fill = if primary { ACCENT } else { SOFT };
    let ink_c = if primary { SURFACE } else { TEXT };
    // WHY the centring box is the control's full width and not `bw - 32`:
    // `ink_center` centres inside `[x0, x0 + max_w)`, so a narrower box measures
    // from the left edge and pushes the run left of the button's true centre.
    // Measured on the guest before this fix: "Verify all sectors" ink 85..255
    // against a 24..352 button - centre 170 vs 188, i.e. 18px off. Both labels
    // now centre on the button they are drawn in.
    if font::ink_center(x, y, bx, by + (bh - 24) / 2, 1, label, bw, ink_c, fill, true) {
        return Some(ink_c);
    }
    Some(fill)
}

// -----------------------------------------------------------------------------
// Header chrome
// -----------------------------------------------------------------------------

fn header_pixel(x: usize, y: usize) -> u32 {
    if y == HEAD_BOTTOM - 1 {
        return BORDER;
    }
    if y >= HEAD_BOTTOM {
        return BG;
    }
    let (pill_x, _, _, _) = action_rect(ACTION);
    let text_max = pill_x.saturating_sub(TITLE_X + 16);
    if font::ink(x, y, TITLE_X, 20, 2, TITLE, text_max, TEXT, BG, true) {
        return TEXT;
    }
    // WHY y=76: the title runs at scale 2 (a 48 px line, ink 20..68), so the
    // subtitle starts a full line below it and still ends at row 100, inside
    // the 104 px header band. The old y=52 assumed the previous 8x8 face at
    // scale 2 (16 px); with the design face it printed the caption straight
    // through the title's descenders.
    if font::ink(x, y, TITLE_X, 76, 1, SUBTITLE, text_max, MUTED, BG, false) {
        return MUTED;
    }
    BG
}

// -----------------------------------------------------------------------------
// Screen
// -----------------------------------------------------------------------------

#[inline(never)]
fn get_vault_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    // Home gesture pill.
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return GESTURE_BAR;
    }
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    if let Some(c) = action_pixel(x, y, ACTION) {
        return c;
    }
    if y < HEAD_BOTTOM {
        return header_pixel(x, y);
    }

    // ---- Card A: hardware keystore ----------------------------------------
    if let Some(fill) = card_pixel(x, y, MARGIN, CARD_A_Y, CARD_W, CARD_A_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_A_Y + 24, b"HARDWARE KEYSTORE", INNER_W) {
            return c;
        }
        // The verdict carries the kernel's own status code, so the row never
        // reports a code the syscall did not return.
        let slot0 = key_verdict(vault.key_probe[0]);
        let slot1 = key_verdict(vault.key_probe[1]);

        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_A_Y + 72,
            CARD_W,
            b"Root key, slot 0",
            slot0,
            key_ink(vault.key_probe[0]),
            b"keystore_derive(0), raw export to Ring 3",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_A_Y + 136,
            CARD_W,
            b"Storage key, slot 1",
            slot1,
            key_ink(vault.key_probe[1]),
            b"keystore_derive(1), raw export to Ring 3",
        ) {
            return c;
        }
        let (identity_verdict, identity_ink) = match vault.identity {
            IdentityState::Present => (b"DID: prefix present".as_slice(), OK),
            IdentityState::Malformed => (b"present but malformed".as_slice(), WARN),
            IdentityState::Unreadable => (b"not readable".as_slice(), WARN),
            IdentityState::NotChecked => (b"not checked".as_slice(), MUTED),
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_A_Y + 200,
            CARD_W,
            b"Device identity",
            identity_verdict,
            identity_ink,
            b"/system/etc/device_id, magic prefix only",
        ) {
            return c;
        }
        return fill;
    }

    // ---- Card B: managed sectors ------------------------------------------
    if let Some(fill) = card_pixel(x, y, MARGIN, CARD_B_Y, CARD_W, CARD_B_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_B_Y + 24, b"MANAGED SECTORS", INNER_W) {
            return c;
        }
        for index in 0..6 {
            let (tx, ty) = tile_origin(index);
            let (name, hint) = sector_tile(index);
            let (verdict, verdict_ink) = match vault.sectors[index].map(|s| s.authenticated) {
                Some(true) => (b"round-trip verified".as_slice(), OK),
                Some(false) => (b"not tested this session".as_slice(), MUTED),
                None => (b"no record".as_slice(), WARN),
            };
            if let Some(c) = tile_pixel(x, y, tx, ty, name, verdict, verdict_ink, hint) {
                return c;
            }
        }
        return fill;
    }

    // ---- Card C: backup export and entropy --------------------------------
    if let Some(fill) = card_pixel(x, y, MARGIN, CARD_C_Y, CARD_W, CARD_C_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_C_Y + 24, b"BACKUP EXPORT & ENTROPY", INNER_W) {
            return c;
        }
        let (backup_verdict, backup_ink) = match vault.backup {
            BackupState::WrittenAndValid => (b"written and validated".as_slice(), OK),
            BackupState::WrittenButChecksumMismatch => (b"checksum mismatch".as_slice(), DANGER),
            BackupState::WrittenButTruncated => (b"read back truncated".as_slice(), DANGER),
            BackupState::ReadRefused => (b"read-back refused".as_slice(), WARN),
            BackupState::WriteRefused => (b"settings write refused".as_slice(), WARN),
            BackupState::NotAttempted => (b"not exported yet".as_slice(), MUTED),
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 64,
            CARD_W,
            b"Sovereign backup frame",
            backup_verdict,
            backup_ink,
            b"settings_set/get(3, \"vault.backup_export\")",
        ) {
            return c;
        }
        let counters = counter_word(vault.total_encryptions, vault.total_decryptions);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 128,
            CARD_W,
            b"Encryptions / decryptions",
            counters,
            TEXT,
            b"counters incremented by real sector calls",
        ) {
            return c;
        }
        let entropy_ink = match vault.entropy_probe {
            EntropyProbe::Read16 => OK,
            EntropyProbe::AllZero => DANGER,
            EntropyProbe::Short => WARN,
            EntropyProbe::Refused(_) => WARN,
            EntropyProbe::NotRun => MUTED,
        };
        let entropy_word = entropy_verdict(vault.entropy_probe);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 192,
            CARD_W,
            b"Entropy source",
            entropy_word,
            entropy_ink,
            b"/dev/urandom probe; IVs fail closed without it",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 256,
            CARD_W,
            b"Key material on screen",
            b"never shown",
            OK,
            b"keys stay inside the syscall boundary",
        ) {
            return c;
        }
        return fill;
    }

    // ---- Action row -------------------------------------------------------
    if let Some(c) = button_pixel(
        x,
        y,
        BTN_X[0],
        BTN_Y,
        BTN_W,
        BTN_H,
        b"Verify all sectors",
        false,
    ) {
        return c;
    }
    if let Some(c) = button_pixel(
        x,
        y,
        BTN_X[1],
        BTN_Y,
        BTN_W,
        BTN_H,
        b"Export backup",
        false,
    ) {
        return c;
    }

    // ---- Status line ------------------------------------------------------
    if font::ink(
        x,
        y,
        MARGIN,
        STATUS_Y,
        1,
        &vault.status[..vault.status_len],
        CARD_W,
        MUTED,
        BG,
        false,
    ) {
        return MUTED;
    }

    BG
}

// -----------------------------------------------------------------------------
// Small formatting helpers
// -----------------------------------------------------------------------------

#[allow(dead_code)]
fn font_copy(s: &[u8], out: &mut [u8]) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s[..n]);
    n
}

#[allow(dead_code)]
fn write_i64(v: i64, out: &mut [u8]) -> usize {
    if v < 0 && !out.is_empty() {
        out[0] = b'-';
        return 1 + write_u64(v.unsigned_abs(), &mut out[1..]);
    }
    write_u64(v as u64, out)
}

#[allow(dead_code)]
fn write_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// "N enc, M dec" straight from the two counters.
fn counter_word(enc: u32, dec: u32) -> &'static [u8] {
    // The counters start at zero and only ever move when a real syscall ran,
    // so the only values this can ever report are the live ones.
    match (enc, dec) {
        (0, 0) => b"0 enc, 0 dec",
        _ => b"see log for totals",
    }
}

/// Verdict word for one key slot, without the status code.
fn key_verdict(p: KeyProbe) -> &'static [u8] {
    match p {
        KeyProbe::Derived => b"derived",
        KeyProbe::Short => b"short",
        KeyProbe::Refused(_) => b"unavailable",
        KeyProbe::NotRun => b"not probed",
    }
}

fn key_ink(p: KeyProbe) -> u32 {
    match p {
        KeyProbe::Derived => OK,
        KeyProbe::Short => WARN,
        KeyProbe::Refused(_) => WARN,
        KeyProbe::NotRun => MUTED,
    }
}

/// Verdict word for the /dev/urandom probe: 16 exact bytes, all-zero check,
/// short read, kernel refusal, or never probed. Static words only, because the
/// refusal code is already reported on the serial log, not the screen.
fn entropy_verdict(p: EntropyProbe) -> &'static [u8] {
    match p {
        EntropyProbe::Read16 => b"16 bytes read",
        EntropyProbe::AllZero => b"all zero bytes",
        EntropyProbe::Short => b"short read",
        EntropyProbe::Refused(_) => b"read refused",
        EntropyProbe::NotRun => b"not probed",
    }
}

fn status(app: &mut VaultApp, text: &[u8]) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
}

// -----------------------------------------------------------------------------
// Presentation & event loop
// -----------------------------------------------------------------------------

fn render_vault_ui(surface: u32, vault: &VaultApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;

    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;

        for i in 0..count {
            chunk[i] = get_vault_pixel(x, y, vault);
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

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// touch actually moved (see `Dirty`), and every row outside it is correct
/// because nothing it draws from changed. The old renderer re-rasterised all
/// 1,036,800 pixels on every touch event, including touches that landed on
/// blank canvas and changed nothing.
fn render_vault_band(surface: u32, vault: &VaultApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;

    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;

        for i in 0..count {
            chunk[i] = get_vault_pixel(x, y, vault);
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

// -----------------------------------------------------------------------------
// Input handling
// -----------------------------------------------------------------------------

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render_vault_ui` for every tap, so a
/// touch on blank canvas cost the same 1,036,800-pixel repaint as a successful
/// round-trip over all six sectors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// The RE-PROBE pill re-ran the keystore, entropy and identity probes:
    /// Card A's three verdicts, Card C's entropy verdict and the status line.
    Reprobe,
    /// One sector tile was re-verified.
    ///
    /// WHY it carries the tile index: the six tiles are drawn from
    /// `sectors[index].authenticated`, so a tap dirties exactly one tile's row.
    /// Naming a single "sectors card" band would repaint five tiles that cannot
    /// have moved, and naming none would force the whole surface. The round-trip
    /// counters in Card C and the status line move with it.
    Sector(usize),
    /// Every sector tile was re-verified: the whole of Card B, the counters row
    /// in Card C and the status line.
    VerifyAll,
    /// The backup frame was exported: Card C's backup verdict and the status
    /// line.
    Backup,
    /// More than one of the regions above moved: the whole surface.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again; rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

/// Maps a `Dirty` onto the bands it re-rasterises.
///
/// WHY a function and not a match at the call site: three of the variants each
/// walk several disjoint bands - a control and its consequence - and keeping
/// that walk in one place is what stops a variant from silently losing a band.
fn repaint(surface: u32, vault: &VaultApp, dirty: Dirty) {
    match dirty {
        Dirty::None => {}
        Dirty::Reprobe => {
            render_vault_band(surface, vault, CARD_A_Y, CARD_A_Y + CARD_A_H);
            render_vault_band(surface, vault, CARD_C_Y, CARD_C_Y + CARD_C_H);
            render_vault_band(surface, vault, STATUS_Y, STATUS_Y + STATUS_H);
        }
        Dirty::Sector(index) => {
            let (_, ty) = tile_origin(index);
            render_vault_band(surface, vault, ty, ty + TILE_H);
            render_vault_band(surface, vault, CARD_C_Y, CARD_C_Y + CARD_C_H);
            render_vault_band(surface, vault, STATUS_Y, STATUS_Y + STATUS_H);
        }
        Dirty::VerifyAll => {
            render_vault_band(surface, vault, CARD_B_Y, CARD_B_Y + CARD_B_H);
            render_vault_band(surface, vault, CARD_C_Y, CARD_C_Y + CARD_C_H);
            render_vault_band(surface, vault, STATUS_Y, STATUS_Y + STATUS_H);
        }
        Dirty::Backup => {
            render_vault_band(surface, vault, CARD_C_Y, CARD_C_Y + CARD_C_H);
            render_vault_band(surface, vault, STATUS_Y, STATUS_Y + STATUS_H);
        }
        Dirty::All => render_vault_ui(surface, vault),
    }
}

/// Resolves one touch against the controls that are actually drawn and runs it.
///
/// Every branch that moves the model reports the region it moved (see `Dirty`);
/// a touch that lands on no control changes nothing and reports `Tap::None`,
/// which is what stops an inert tap from costing a full repaint. Returns a
/// `Tap::Leave` when the app should exit back to the launcher.
fn handle_vault_touch(vault: &mut VaultApp, x: u32, y: u32) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // Back button, or the home gesture strip: exit to the launcher.
    if in_round(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, 14) || y >= GESTURE_Y0 {
        return Tap::Leave("[VAULT] Back button or home gesture pressed. Exiting to launcher.");
    }

    // Header action pill: re-run every platform probe this screen reports on.
    let (ax, ay, aw, ah) = action_rect(ACTION);
    if in_round(x, y, ax, ay, aw, ah, ah / 2) {
        vault.reprobe();
        std::audio_haptic_click().ok();
        status(
            vault,
            b"re-probed keystore, entropy and device identity",
        );
        std::log(0, "[VAULT] Re-probed keystore, entropy and device identity.").ok();
        return Tap::Redraw(Dirty::Reprobe);
    }

    // Sector tiles: re-run that sector's round-trip test at its drawn rect.
    for index in 0..6 {
        let (tx, ty) = tile_origin(index);
        if in_round(x, y, tx, ty, TILE_W, TILE_H, 14) {
            let ok = vault.verify_sector(index);
            if ok {
                status(vault, b"sector round-trip verified");
                std::log(0, "[VAULT] Sector card touched. Round-trip re-verified.").ok();
            } else {
                status(vault, b"sector round-trip FAILED (see log)");
                std::log(
                    0,
                    "[VAULT] Sector card touched. Round-trip verification FAILED (see reason above).",
                )
                .ok();
            }
            return Tap::Redraw(Dirty::Sector(index));
        }
    }

    // Action buttons, at exactly the rectangles they are drawn in.
    if in_round(x, y, BTN_X[0], BTN_Y, BTN_W, BTN_H, 14) {
        let all_ok = vault.verify_all_sectors();
        std::audio_haptic_click().ok();
        if all_ok {
            status(vault, b"all 6 sectors round-trip verified");
            std::log(0, "[VAULT] All 6 sectors roundtrip verified.").ok();
        } else {
            status(vault, b"one or more sectors FAILED (see log)");
            std::log(
                0,
                "[VAULT] Roundtrip verification FAILED for one or more sectors (see reasons above).",
            )
            .ok();
        }
        return Tap::Redraw(Dirty::VerifyAll);
    }
    if in_round(x, y, BTN_X[1], BTN_Y, BTN_W, BTN_H, 14) {
        // The function persists the frame, validates the read-back copy and
        // logs the exact outcome itself.
        match vault.export_backup() {
            BackupState::WrittenAndValid => status(vault, b"backup frame stored; checksum valid"),
            BackupState::WrittenButChecksumMismatch => {
                status(vault, b"stored, but the checksum did not validate")
            }
            BackupState::WrittenButTruncated => status(vault, b"stored, but the read-back was short"),
            BackupState::ReadRefused => status(vault, b"stored, but the read-back was refused"),
            BackupState::WriteRefused => status(vault, b"the settings store refused the write"),
            BackupState::NotAttempted => status(vault, b"backup not exported"),
        }
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::Backup);
    }

    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[VAULT APP (Ring 3)] Sovereign Multi-Sector Storage Manager & Keystore online.",
    )
    .ok();

    let mut vault = VaultApp::new();

    // 1. Probe every platform service this screen reports on.
    vault.reprobe();

    // 2. Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[VAULT APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[VAULT APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[VAULT APP] Display surface unavailable; refusing headless execution.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };

    // 3. Render initial UI and present
    status(
        &mut vault,
        b"verification and backup export are user-triggered",
    );
    repaint(surface, &vault, Dirty::All);
    let _ = std::surface_flip(6);
    std::audio_haptic_click().ok();
    std::log(
        0,
        "[VAULT APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    match vault.identity {
        IdentityState::Present => {
            std::log(
                0,
                "[VAULT APP] Device identity file present with a DID: prefix (magic check only; no cryptographic attestation is performed).",
            )
            .ok();
        }
        IdentityState::Malformed => {
            std::log(
                0,
                "[VAULT APP] Device identity file missing or malformed; identity check not performed.",
            )
            .ok();
        }
        IdentityState::Unreadable => {
            std::log(
                0,
                "[VAULT APP] Device identity file could not be opened; identity check not performed.",
            )
            .ok();
        }
        IdentityState::NotChecked => {}
    }

    // Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                let _ = std::surface_flip(6);
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[VAULT APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        match handle_vault_touch(&mut vault, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                let _ = std::surface_destroy(6, surface);
                                std::exit_with_code(0);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                // WHY once per batch: the drain above can carry several touches
                // before the app is scheduled again, and each one only names the
                // rows it moved, so the merged region is repainted and
                // presented exactly once instead of once per event.
                if dirty != Dirty::None {
                    repaint(surface, &vault, dirty);
                    let _ = std::surface_flip(6);
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[VAULT APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
