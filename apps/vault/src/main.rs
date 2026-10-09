//! Inter OS Native Sovereign Vault & Multi-Sector Storage Manager App
//!
//! Full production encrypted storage manager executing in Ring 3 (User Mode):
//! - Hardware Keystore cryptographic key derivation across multiple security slots:
//!   * Slot 0: Sovereign Root Hardware Master Key
//!   * Slot 1: General Media & Container Storage Key
//!   * Slot 2: Optical Sensor & Neural Vision Storage Key
//!   * Slot 3: End-to-End P2P Wire Messaging Key
//! - Multi-Sector Partition Indexing & Management:
//!   * Sector 100: Sovereign Root Keystore & Identity Certificate
//!   * Sector 200: General Media Containers & Documents
//!   * Sector 201: Camera Photos & Video ISP Metadata
//!   * Sector 202: Encrypted Messages & P2P Wire Ledger
//!   * Sector 203: Calendar Events & Agenda Timeline
//!   * Sector 204: Audio Tracks & Music Studio Compositions
//! - Hardware TRNG Entropy Ingestion (/dev/urandom) for Initialization Vectors
//! - Authenticated AES-XTS & ChaCha20-Poly1305 sector round-trip verification
//! - Device identity magic check via VFS (`DID:` prefix only, no attestation)
//! - Sovereign Backup Export framing persisted through the kernel settings
//!   store with a Fletcher-32 checksum, validated by reading it back
//! - DRM/KMS Client Surface Rendering (z=200) with Sector Partition Browser & Enclave Status
//! - Asynchronous event reactor loop handling storage IPC requests & maintenance timers

#![no_std]
#![no_main]

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
    pub master_key_derived: bool,
    pub media_key_derived: bool,
    pub total_encryptions: u32,
    pub total_decryptions: u32,
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
            master_key_derived: false,
            media_key_derived: false,
            total_encryptions: 0,
            total_decryptions: 0,
        }
    }

    /// Derives cryptographic keys from the hardware keystore.
    pub fn derive_keys(&mut self) {
        let mut key0 = [0u8; 32];
        if let Ok(len) = std::keystore_derive(0, &mut key0) {
            if len == 32 {
                self.master_key_derived = true;
                std::log(
                    0,
                    "[VAULT APP] Hardware Keystore Slot 0 derived: Root Master Key online.",
                )
                .ok();
            }
        }

        let mut key1 = [0u8; 32];
        if let Ok(len) = std::keystore_derive(1, &mut key1) {
            if len == 32 {
                self.media_key_derived = true;
                std::log(
                    0,
                    "[VAULT APP] Hardware Keystore Slot 1 derived: Storage Encryption Key online.",
                )
                .ok();
            }
        }
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

    /// Runs roundtrip verification across all managed sectors.
    /// Returns true only when every sector's roundtrip actually succeeded.
    pub fn verify_all_sectors(&mut self) -> bool {
        // Not short-circuited on purpose: every sector must be exercised even
        // after an early failure, so the per-sector failure logs are complete.
        let mut all_ok = true;
        all_ok &= self.test_sector_roundtrip(100, b"INTER_ROOT_KEYSTORE_RECORD_V1");
        all_ok &= self.test_sector_roundtrip(200, b"INTER_MEDIA_CONTAINER_RECORD_V1");
        all_ok &= self.test_sector_roundtrip(201, b"INTER_CAMERA_PHOTO_RECORD_V1");
        all_ok &= self.test_sector_roundtrip(202, b"INTER_P2P_MESSAGE_RECORD_V1");
        all_ok &= self.test_sector_roundtrip(203, b"INTER_CALENDAR_EVENT_RECORD_V1");
        all_ok &= self.test_sector_roundtrip(204, b"INTER_AUDIO_TRACK_RECORD_V1");
        all_ok
    }

    /// Builds a sovereign backup export frame, persists it through the kernel
    /// settings store, and validates the stored frame by reading it back.
    ///
    /// WHY: this previously built the frame on the stack, dropped it, and
    /// logged "generated & verified (Fletcher-32 valid)" unconditionally —
    /// nothing was written anywhere, so nothing could be verified. The frame
    /// is now persisted as hex (the settings store rejects newline bytes and
    /// raw counter/checksum bytes can contain 0x0A) under `vault.backup_export`
    /// via FileSystemWrite at cap slot 3, and "verified" means the read-back
    /// frame carries the expected magic and its stored Fletcher-32 checksum
    /// equals the recomputed one.
    pub fn export_backup(&mut self) -> bool {
        let mut header = [0u8; 40];
        header[..28].copy_from_slice(b"INTER_SOVEREIGN_VAULT_BACKUP");
        header[28..32].copy_from_slice(&self.total_encryptions.to_le_bytes());
        header[32..36].copy_from_slice(&self.total_decryptions.to_le_bytes());
        let sum = fletcher32(&header[..36]);
        header[36..40].copy_from_slice(&sum.to_le_bytes());

        let mut encoded = [0u8; 80];
        let enc_len = hex_encode(&header, &mut encoded);
        if std::settings_set(3, "vault.backup_export", &encoded[..enc_len]).is_err() {
            std::log(
                0,
                "[VAULT APP] Backup export refused: the settings store rejected the write.",
            )
            .ok();
            return false;
        }

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
                        true
                    } else {
                        std::log(
                            0,
                            "[VAULT APP] Backup Export stored, but the read-back frame failed checksum validation.",
                        )
                        .ok();
                        false
                    }
                }
                _ => {
                    std::log(
                        0,
                        "[VAULT APP] Backup Export stored, but the read-back frame was truncated or undecodable; validation not performed.",
                    )
                    .ok();
                    false
                }
            },
            _ => {
                std::log(
                    0,
                    "[VAULT APP] Backup Export stored, but the read-back was refused; validation not performed.",
                )
                .ok();
                false
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF080D1A;
const SURFACE_DARK: u32 = 0xFF101726;
const SURFACE_RAISED: u32 = 0xFF192236;
const BORDER_SUBTLE: u32 = 0xFF2A374F;
const ACCENT_SKY: u32 = 0xFF38BDF8;
const ACCENT_AMBER: u32 = 0xFFFFB800;
const SUCCESS_EMERALD: u32 = 0xFF10B981;
const TEXT_PRIMARY: u32 = 0xFFF8FAFC;
const TEXT_MUTED: u32 = 0xFF94A3B8;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

/// Vector padlock icon for sector security status
fn test_vault_padlock(x: usize, y: usize, cx: usize, cy: usize, locked: bool) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    // Shackle loop (top)
    if dy <= -2 && dy >= -16 {
        let shackle_offset = if locked { 0 } else { -4 };
        let r2 = (dx - shackle_offset) * (dx - shackle_offset) + (dy + 8) * (dy + 8);
        if r2 >= 5 * 5 && r2 <= 9 * 9 && dy <= -8 {
            return true;
        }
        if (dx - shackle_offset).abs() >= 5 && (dx - shackle_offset).abs() <= 9 && dy >= -8 && dy <= -2 {
            return true;
        }
    }
    // Solid padlock body
    if dy >= -2 && dy <= 14 && dx.abs() <= 11 {
        // Keyhole aperture
        let is_keyhole = (dx.abs() <= 1 && dy >= 4 && dy <= 9) || (dx * dx + (dy - 4) * (dy - 4) <= 4);
        if is_keyhole {
            return false;
        }
        return true;
    }
    false
}

const SECTOR_CARD_Y: [usize; 6] = [290, 420, 550, 680, 810, 940];
const SECTOR_CARD_H: usize = 118;

#[inline(never)]
fn get_vault_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Vault"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 116, 36, 3, b"Vault", TEXT_PRIMARY) {
                return c;
            }
        }

        // Hardware Keystore Badge (x: 460..696, y: 28..68)
        if x >= 460 && x < 696 && y >= 28 && y < 68 {
            let rx = (x - 460).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            let is_active = vault.master_key_derived && vault.media_key_derived;
            let (status_text, status_color) = if is_active {
                (&b"KEYS DERIVED"[..], SUCCESS_EMERALD)
            } else {
                (&b"UNAVAILABLE"[..], TEXT_MUTED)
            };

            if let Some(c) = test_text(x, y, 480, 42, 2, status_text, status_color) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return status_color;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 2. Storage Overview Card (y: 110..270, x: 24..696)
    if (110..270).contains(&y) && x >= 24 && x < W - 24 {
        let rx = (x - 24).min(W - 25 - x);
        let ry = (y - 110).min(269 - y);
        if rx < 18 && ry < 18 {
            let cdx = (18 - rx) as isize;
            let cdy = (18 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 324 {
                return BG_DARK;
            }
        }

        if let Some(c) = test_text(x, y, 44, 126, 2, b"HARDWARE ENCRYPTED STORAGE", TEXT_PRIMARY) {
            return c;
        }
        if let Some(c) = test_text(x, y, 44, 150, 1, b"Cryptographic verification is determined by runtime checks", TEXT_MUTED) {
            return c;
        }

        // Each segment reflects the corresponding sector's actual last
        // round-trip verification result; it is not a storage-utilization bar.
        if (176..206).contains(&y) && x >= 44 && x < W - 44 {
            let rel_x = x - 44;
            let seg_w = (W - 88) / 6;
            let seg_idx = (rel_x / seg_w).min(5);
            let in_seg_x = rel_x % seg_w;
            if in_seg_x >= seg_w - 3 {
                return SURFACE_DARK;
            }
            let verified = vault.sectors[seg_idx].map_or(false, |s| s.authenticated);
            return if verified { SUCCESS_EMERALD } else { TEXT_MUTED };
        }

        if let Some(c) = test_text(x, y, 44, 222, 1, b"Sector verification is shown on each partition", ACCENT_SKY) {
            return c;
        }
        let key_status: &[u8] = match (vault.master_key_derived, vault.media_key_derived) {
            (true, true) => &b"Root and storage keys derived"[..],
            (true, false) => &b"Root key derived; storage key unavailable"[..],
            (false, true) => &b"Storage key derived; root key unavailable"[..],
            (false, false) => &b"Root and storage keys unavailable"[..],
        };
        if let Some(c) = test_text(x, y, 44, 240, 1, key_status, TEXT_MUTED) {
            return c;
        }

        if rx < 2 || ry < 2 {
            return BORDER_SUBTLE;
        }
        return SURFACE_DARK;
    }

    // 3. Sector Cards (y: 290, 420, 550, 680, 810, 940)
    for idx in 0..6 {
        let cy = SECTOR_CARD_Y[idx];
        if y >= cy && y < cy + SECTOR_CARD_H && x >= 24 && x < W - 24 {
            let rx = (x - 24).min(W - 25 - x);
            let ry = (y - cy).min(SECTOR_CARD_H - 1 - (y - cy));
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            // Left Sector Badge Box (x: 40..144, y: cy + 18..cy + 100)
            if x >= 40 && x < 144 && y >= cy + 18 && y < cy + 100 {
                let bx = (x - 40).min(143 - x);
                let by = (y - (cy + 18)).min(cy + 99 - y);
                if bx < 12 && by < 12 {
                    let cdx = (12 - bx) as isize;
                    let cdy = (12 - by) as isize;
                    if cdx * cdx + cdy * cdy >= 144 {
                        return SURFACE_DARK;
                    }
                }

                let sec_label = match idx {
                    0 => &b"S100"[..],
                    1 => &b"S200"[..],
                    2 => &b"S201"[..],
                    3 => &b"S202"[..],
                    4 => &b"S203"[..],
                    _ => &b"S204"[..],
                };

                if let Some(c) = test_text(x, y, 54, cy + 50, 2, sec_label, ACCENT_SKY) {
                    return c;
                }

                if bx < 2 || by < 2 {
                    return BORDER_SUBTLE;
                }
                return SURFACE_RAISED;
            }

            // Middle Description & Details
            let (desc, sub) = match idx {
                0 => (&b"Root Keystore & Identity"[..], &b"Logical sector 100"[..]),
                1 => (&b"General Media & Documents"[..], &b"Logical sector 200"[..]),
                2 => (&b"Camera Photos & ISP Vision"[..], &b"Logical sector 201"[..]),
                3 => (&b"Encrypted Messages & P2P"[..], &b"Logical sector 202"[..]),
                4 => (&b"Calendar Events & Agenda"[..], &b"Logical sector 203"[..]),
                _ => (&b"Audio Tracks & Studio"[..], &b"Logical sector 204"[..]),
            };

            if let Some(c) = test_text(x, y, 160, cy + 32, 2, desc, TEXT_PRIMARY) {
                return c;
            }
            if let Some(c) = test_text(x, y, 160, cy + 64, 1, sub, TEXT_MUTED) {
                return c;
            }

            // Right Padlock Security Status
            let is_auth = vault.sectors[idx].map_or(false, |s| s.authenticated);
            let pad_color = if is_auth { SUCCESS_EMERALD } else { ACCENT_AMBER };

            if test_vault_padlock(x, y, 630, cy + 46, true) {
                return pad_color;
            }

            // The flag records a successful in-memory roundtrip, not stored
            // data integrity, so the label says what was actually tested.
            let pad_label = if is_auth { &b"TESTED"[..] } else { &b"UNTESTED"[..] };
            let pad_lx = if is_auth { 608 } else { 600 };
            if let Some(c) = test_text(x, y, pad_lx, cy + 74, 1, pad_label, pad_color) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_DARK;
        }
    }

    // 4. Action Buttons (y: 1080..1220)
    if (1080..1220).contains(&y) {
        // Button 1: Test All Roundtrips (x: 40..340, y: 1100..1190)
        if x >= 40 && x < 340 && y >= 1100 && y < 1190 {
            let rx = (x - 40).min(339 - x);
            let ry = (y - 1100).min(1189 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return BG_DARK;
                }
            }

            if let Some(c) = test_text(x, y, 116, 1136, 2, b"VERIFY ALL", ACCENT_SKY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return ACCENT_SKY;
            }
            return SURFACE_RAISED;
        }

        // Button 2: Export Backup (x: 380..680, y: 1100..1190)
        if x >= 380 && x < 680 && y >= 1100 && y < 1190 {
            let rx = (x - 380).min(679 - x);
            let ry = (y - 1100).min(1189 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return BG_DARK;
                }
            }

            if let Some(c) = test_text(x, y, 460, 1136, 2, b"EXPORT BACKUP", SUCCESS_EMERALD) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return SUCCESS_EMERALD;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 5. Bottom Navigation Dock (y: 1260..1400)
    if (1260..1400).contains(&y) && x >= 24 && x < W - 24 {
        if y == 1260 {
            return BORDER_SUBTLE;
        }

        // Tab 0: Sectors (Active)
        if let Some(c) = test_text(x, y, 80, 1318, 2, b"SECTORS", ACCENT_SKY) {
            return c;
        }
        if x >= 74 && x < 206 && y >= 1344 && y < 1348 {
            return ACCENT_SKY;
        }

        // WHY: the former KEYSTORE and AUDIT dock labels had no tap handlers
        // and no screens behind them; they implied navigable tabs that never
        // existed, so they were removed rather than left as dead controls.

        return SURFACE_DARK;
    }

    // Home Gesture Bar
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

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

/// Returns true if app should exit back to Launcher.
fn handle_vault_touch(vault: &mut VaultApp, x: u32, y: u32) -> bool {
    let x = x as usize;
    let y = y as usize;

    // 1. Back button in header or Home gesture bar: exit to Launcher
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        std::log(
            0,
            "[VAULT] Back button or home gesture pressed. Exiting to launcher.",
        )
        .ok();
        return true;
    }

    // 2. Sector Cards tap: re-verify target sector
    for idx in 0..6 {
        let cy = SECTOR_CARD_Y[idx];
        if y >= cy && y < cy + SECTOR_CARD_H && x >= 24 && x < W - 24 {
            if let Some(sec) = vault.sectors[idx] {
                let sector_id = sec.sector_id;
                let roundtrip_ok =
                    vault.test_sector_roundtrip(sector_id, b"INTER_VAULT_TOUCH_AUTH_VERIFY");
                std::audio_haptic_click().ok();
                if roundtrip_ok {
                    std::log(0, "[VAULT] Sector card touched. Round-trip re-verified.").ok();
                } else {
                    std::log(
                        0,
                        "[VAULT] Sector card touched. Round-trip verification FAILED (see reason above).",
                    )
                    .ok();
                }
            }
            return false;
        }
    }

    // 3. Action Buttons (hit boxes match the drawn buttons at y 1100..1190)
    if (1080..1220).contains(&y) {
        if x >= 40 && x < 340 && y >= 1100 && y < 1190 {
            // Test All Roundtrips
            let all_ok = vault.verify_all_sectors();
            std::audio_haptic_click().ok();
            if all_ok {
                std::log(0, "[VAULT] All 6 sectors roundtrip verified.").ok();
            } else {
                std::log(
                    0,
                    "[VAULT] Roundtrip verification FAILED for one or more sectors (see reasons above).",
                )
                .ok();
            }
            return false;
        } else if x >= 380 && x < 680 && y >= 1100 && y < 1190 {
            // Export Backup: the function persists the frame, validates the
            // read-back copy, and logs the exact outcome itself.
            let _ = vault.export_backup();
            std::audio_haptic_click().ok();
            return false;
        }
    }

    false
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[VAULT APP (Ring 3)] Sovereign Multi-Sector Storage Manager & Keystore online.",
    )
    .ok();

    let mut vault = VaultApp::new();

    // 1. Derive Cryptographic Keys from Hardware Keystore
    vault.derive_keys();

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
    render_vault_ui(surface, &vault);
    let _ = std::surface_flip(6);
    std::audio_haptic_click().ok();
    std::log(
        0,
        "[VAULT APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // Verification and backup export are user-triggered actions.

    // Verify Sovereign Device Certificate from VFS
    if let Ok(fd) = std::fs_open(3, "/system/etc/device_id", std::FS_READ) {
        let mut dev_buf = [0u8; 64];
        if let Ok(n) = std::fs_read(3, fd, &mut dev_buf) {
            if n >= 4 && &dev_buf[..4] == b"DID:" {
                // WHY: only the magic prefix is checked here; with no
                // signature or digest to compare against, this is a presence
                // check, not an authentication.
                std::log(
                    0,
                    "[VAULT APP] Device identity file present with a DID: prefix (magic check only; no cryptographic attestation is performed).",
                )
                .ok();
            } else {
                std::log(
                    0,
                    "[VAULT APP] Device identity file missing or malformed; identity check not performed.",
                )
                .ok();
            }
        }
        let _ = std::fs_close(fd);
    } else {
        std::log(
            0,
            "[VAULT APP] Device identity file could not be opened; identity check not performed.",
        )
        .ok();
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
                        if handle_vault_touch(&mut vault, input.x, input.y) {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        render_vault_ui(surface, &vault);
                        let _ = std::surface_flip(6);
                    }
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
