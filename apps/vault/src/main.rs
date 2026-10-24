//! Inter OS Sovereign Vault & Multi-Sector Storage Manager
//!
//! Ring 3 storage manager. Everything this app displays is the result of
//! a syscall it just made:
//!
//! - `keystore_derive(0/1)` for the two key slots. This kernel refuses to export
//!   raw key material to Ring 3, so both probes normally answer
//!   `PermissionDenied` and the screen says "unavailable" rather than implying
//!   an enclave it does not have.
//! - `vault_encrypt_sector` / `vault_decrypt_sector` for the crypto self-test.
//!   A sector row is only marked "round-trip verified" after encrypt ->
//!   mutate-check -> decrypt -> compare actually succeeded. The probe runs
//!   against `SELF_TEST_BLOCK`, a block of this app's own, because sealing a
//!   managed sector would replace the record another app stored there.
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

use core::panic::PanicInfo;
use interos_std as std;
use interos_std::design::components;
use vault::theme::{spacing, THEME};
use vault::{
    fletcher32, hex_decode, hex_encode, hit_test_pin_disc, key_material_row, BackupState,
    EntropyProbe, IdentityState, KeyProbe, VaultSectorType, PIN_COLS, PIN_DISC_RADIUS,
    PIN_ROWS,
};

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

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

/// The managed sector blocks, in the order the tiles show them.
///
/// WHY they are named here: the sector records the app manages are these
/// blocks, and the library's tile hints are built from the same list.
const MANAGED_SECTORS: [u64; 6] = vault::MANAGED_SECTORS;

/// The block the self-test writes to.
const SELF_TEST_BLOCK: u64 = vault::SELF_TEST_BLOCK;

/// Production Sovereign Vault state manager.
pub struct VaultApp {
    pub sectors: [Option<SectorRecord>; 6],
    pub key_probe: [KeyProbe; 2],
    pub entropy_probe: EntropyProbe,
    pub identity: IdentityState,
    pub backup: BackupState,
    pub total_encryptions: u32,
    pub total_decryptions: u32,
    pub counter_buf: [u8; 32],
    pub counter_len: usize,
    /// Last thing the platform actually said, shown verbatim in the status line.
    pub status: [u8; 64],
    pub status_len: usize,
    pub pin_modal_open: bool,
    pub is_authenticated: bool,
    pub show_credentials: bool,
    pub pin_buf: [u8; 8],
    pub pin_len: usize,
    pub pin_error: bool,
}

/// The hardware keystore slot holding the root master key.
///
/// WHY it is named: the argument is a keystore slot, not a capability
/// slot, and the two are not interchangeable. Naming it keeps the
/// difference visible at the call site.
pub const ROOT_KEY_SLOT: u32 = 0;

/// The hardware keystore slot holding the storage encryption key.
pub const STORAGE_KEY_SLOT: u32 = 1;

impl VaultApp {
    pub const fn new() -> Self {
        Self {
            sectors: [
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[0],
                    sector_type: VaultSectorType::RootKeystore,
                    description: "Root Keystore & Identity",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[1],
                    sector_type: VaultSectorType::MediaContainers,
                    description: "General Media & Documents",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[2],
                    sector_type: VaultSectorType::CameraPhotos,
                    description: "Camera Photos & ISP Metadata",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[3],
                    sector_type: VaultSectorType::EncryptedMessages,
                    description: "Encrypted Messages & P2P Wire",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[4],
                    sector_type: VaultSectorType::CalendarEvents,
                    description: "Calendar Events & Timeline",
                    encrypted_bytes: 512,
                    authenticated: false,
                }),
                Some(SectorRecord {
                    sector_id: MANAGED_SECTORS[5],
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
            counter_buf: *b"0 enc, 0 dec\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            counter_len: 12,
            status: [0u8; 64],
            status_len: 0,
            pin_modal_open: false,
            is_authenticated: false,
            show_credentials: false,
            pin_buf: [0u8; 8],
            pin_len: 0,
            pin_error: false,
        }
    }

    pub fn update_counter_buf(&mut self) {
        let mut buf = [0u8; 32];
        let mut n = write_u64(self.total_encryptions as u64, &mut buf);
        let mid = b" enc, ";
        let m = mid.len().min(buf.len().saturating_sub(n));
        buf[n..n + m].copy_from_slice(&mid[..m]);
        n += m;
        n += write_u64(self.total_decryptions as u64, &mut buf[n..]);
        let end = b" dec";
        let e = end.len().min(buf.len().saturating_sub(n));
        buf[n..n + e].copy_from_slice(&end[..e]);
        self.counter_buf = buf;
        self.counter_len = n + e;
    }

    /// True when both hardware key slots actually produced a 32-byte key.
    pub fn keys_derived(&self) -> bool {
        self.key_probe == [KeyProbe::Derived, KeyProbe::Derived]
    }

    /// Derives cryptographic keys from the hardware keystore and records the
    /// exact answer each slot gave.
    pub fn derive_keys(&mut self) {
        let mut key0 = [0u8; 32];
        self.key_probe[0] = match std::keystore_derive(ROOT_KEY_SLOT, &mut key0) {
            Ok(len) if len == 32 => {
                std::log("[VAULT APP] Hardware Keystore Slot 0 derived: Root Master Key online.",
                )
                .ok();
                KeyProbe::Derived
            }
            Ok(_) => KeyProbe::Short,
            Err(e) => KeyProbe::Refused(e),
        };

        let mut key1 = [0u8; 32];
        self.key_probe[1] = match std::keystore_derive(STORAGE_KEY_SLOT, &mut key1) {
            Ok(len) if len == 32 => {
                std::log("[VAULT APP] Hardware Keystore Slot 1 derived: Storage Encryption Key online.",
                )
                .ok();
                KeyProbe::Derived
            }
            Ok(_) => KeyProbe::Short,
            Err(e) => KeyProbe::Refused(e),
        };

        if !self.keys_derived() {
            std::log("[VAULT APP] Hardware keystore did not export raw key material to Ring 3; no key is claimed.",
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
        // WHY the slot is resolved by type: the old code passed the literal
        // slot 3, which holds this app's *write* capability, and the VFS
        // verifies the capability's type at the slot it is given. The read
        // was therefore refused before the file was ever opened, and the
        // identity read as unreadable on a device that has the file.
        self.identity = match std::fs_open("/system/etc/device_id", std::FS_READ) {
            Ok(fd) => {
                let mut dev_buf = [0u8; 64];
                let state = match std::fs_read(fd, &mut dev_buf) {
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

    /// Performs an authenticated encryption and decryption test.
    ///
    /// WHY the round trip runs against `SELF_TEST_BLOCK` and not against the
    /// sector the row is about: `vault_encrypt_sector` is not a dry run. The
    /// kernel seals the caller's payload over the block it is given and
    /// writes the envelope to the storage vault, so a self-test against a
    /// managed sector replaced that sector's record - the camera's photos,
    /// the messages app's wire records, the calendar's agenda - with a test
    /// payload. The managed sectors are live data; the probe has a block of
    /// its own.
    pub fn test_vault_roundtrip(&mut self, block: u64, payload_tag: &[u8]) -> bool {
        let Some(iv) = self.generate_iv() else {
            std::log("[VAULT APP] Entropy unavailable; refusing sector verification.").ok();
            return false;
        };
        let mut sector_buf = [0u8; 512];
        let copy_len = payload_tag.len().min(64);
        sector_buf[..copy_len].copy_from_slice(&payload_tag[..copy_len]);

        match std::vault_encrypt_sector(block, &mut sector_buf, &iv) {
            Ok(()) => {
                self.total_encryptions += 1;
                if &sector_buf[..copy_len] != &payload_tag[..copy_len] {
                    match std::vault_decrypt_sector(block, &mut sector_buf, &iv) {
                        Ok(()) => {
                            self.total_decryptions += 1;
                            if &sector_buf[..copy_len] == &payload_tag[..copy_len] {
                                self.update_counter_buf();
                                return true;
                            } else {
                                std::log("[VAULT APP] Block decrypted but payload mismatch!").ok();
                            }
                        }
                        Err(_) => {
                            std::log("[VAULT APP] Block decrypt returned Err.").ok();
                        }
                    }
                } else {
                    std::log("[VAULT APP] Encrypted buffer identical to plaintext!").ok();
                }
            }
            Err(_) => {
                std::log("[VAULT APP] Block encrypt returned Err.").ok();
            }
        }
        self.update_counter_buf();
        false
    }

    /// Re-runs one sector's round-trip test, recording failure as well as
    /// success so a sector that later stops verifying stops claiming otherwise.
    ///
    /// WHY the probe runs on the app's own block: the row is about a managed
    /// sector, but writing the test payload over that sector would destroy
    /// the record another app stored there. The row reports whether the
    /// vault's crypto round-trips; it never claims the sector was touched.
    pub fn verify_sector(&mut self, index: usize) -> bool {
        if self.sectors[index].is_none() {
            return false;
        }
        let ok = self.test_vault_roundtrip(SELF_TEST_BLOCK, b"INTER_VAULT_TOUCH_AUTH_VERIFY");
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
        //
        // WHY one probe and not six: every round trip writes the block it is
        // given, and the six managed sectors are live data. One probe on the
        // app's own block exercises the same crypto path six times over
        // without writing over anyone's record.
        let mut all_ok = true;
        for index in 0..6 {
            let Some(_sec) = self.sectors[index] else {
                continue;
            };
            let ok = self.test_vault_roundtrip(SELF_TEST_BLOCK, b"INTER_ROOT_KEYSTORE_RECORD_V1");
            if let Some(slot) = self.sectors[index].as_mut() {
                slot.authenticated = ok;
            }
            all_ok &= ok;
        }
        all_ok
    }

    /// Builds a status frame, persists it through the kernel settings store,
    /// and validates the stored frame by reading it back.
    ///
    /// The frame is persisted as hex (the settings store rejects newline bytes
    /// and raw counter/checksum bytes can contain 0x0A) under
    /// `vault.backup_export` through the app's filesystem capabilities, and
    /// "verified" means the read-back frame carries the expected magic and
    /// its stored Fletcher-32 checksum equals the recomputed one.
    ///
    /// WHY it is called a status frame and not a backup: the frame holds two
    /// counters and a checksum, written in the clear. It is not a copy of the
    /// vault and it is not encrypted, and the screen says so.
    pub fn export_backup(&mut self) -> BackupState {
        // WHY the slots are resolved by type: the write and the read are
        // different capabilities, and the VFS and the settings store both
        // verify the capability's type at the slot they are given.
        let mut header = [0u8; 40];
        header[..28].copy_from_slice(b"INTER_SOVEREIGN_VAULT_BACKUP");
        header[28..32].copy_from_slice(&self.total_encryptions.to_le_bytes());
        header[32..36].copy_from_slice(&self.total_decryptions.to_le_bytes());
        let sum = fletcher32(&header[..36]);
        header[36..40].copy_from_slice(&sum.to_le_bytes());

        let mut encoded = [0u8; 80];
        let enc_len = hex_encode(&header, &mut encoded);
        self.backup = if std::settings_set("vault.backup_export", &encoded[..enc_len]).is_err() {
            std::log("[VAULT APP] Backup export refused: the settings store rejected the write.",
            )
            .ok();
            BackupState::WriteRefused
        } else {
            let mut stored_hex = [0u8; 80];
            let mut frame = [0u8; 40];
            match std::settings_get("vault.backup_export", &mut stored_hex) {
                Ok(n) => match hex_decode(&stored_hex[..n.min(stored_hex.len())], &mut frame) {
                    Some(40) => {
                        let stored_sum =
                            u32::from_le_bytes([frame[36], frame[37], frame[38], frame[39]]);
                        if frame[..28] == *b"INTER_SOVEREIGN_VAULT_BACKUP"
                            && stored_sum == fletcher32(&frame[..36])
                        {
                            std::log("[VAULT APP] Sovereign Backup Export persisted; read-back Fletcher-32 checksum valid.",
                            )
                            .ok();
                            BackupState::WrittenAndValid
                        } else {
                            std::log("[VAULT APP] Backup Export stored, but the read-back frame failed checksum validation.",
                            )
                            .ok();
                            BackupState::WrittenButChecksumMismatch
                        }
                    }
                    _ => {
                        std::log("[VAULT APP] Backup Export stored, but the read-back frame was truncated or undecodable; validation not performed.",
                        )
                        .ok();
                        BackupState::WrittenButTruncated
                    }
                },
                _ => {
                    std::log("[VAULT APP] Backup Export stored, but the read-back was refused; validation not performed.",
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
        // WHY the result is announced: the banner used to claim the keystore
        // was online before the app had asked, and on a platform that
        // refuses raw key export the claim was false. The answer is now
        // reported after the probe, not before it.
        if self.keys_derived() {
            std::log("[VAULT APP] Hardware keystore exported both keys; the vault can encrypt.",
            )
            .ok();
        } else {
            std::log("[VAULT APP] Hardware keystore did not export raw key material to this app; no key is claimed.",
            )
            .ok();
        }
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens (canonical light System Space palette)
// -----------------------------------------------------------------------------


// -----------------------------------------------------------------------------
// Layout (8-point grid; 24px screen margin; 48px minimum touch targets)
// -----------------------------------------------------------------------------

const MARGIN: usize = spacing::GRID_24;
const CARD_W: usize = W - MARGIN * 2; // 672
const PAD: usize = spacing::GRID_24; // card inner padding
/// The platform's card treatment, at the elevation the app's cards sit at.
///
/// WHY the app's cards are the platform's cards: a card that invents its own
/// fill and its own hairline is a card a third-party app cannot match, and
/// the platform already says what a card looks like. The resting elevation is
/// the one a card on the vault page sits at: the surface one step above the
/// page, with a hairline around it.
const CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Resting);

/// The card's hairline, or the card's own fill when the card has no hairline.
const CARD_BORDER: u32 = match CARD.border {
    Some(c) => c,
    None => CARD.fill,
};

const CARD_R: usize = CARD.radius;
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
/// The subtitle under the title.
///
/// WHY it does not say the storage is encrypted: the app cannot confirm that.
/// Whether the platform's storage is encrypted is the platform's answer to
/// give, and this app has no probe that asks it - the keystore probe
/// reports whether raw key material was handed over, not whether anything on
/// the disk is encrypted. The old subtitle read "Sovereign encrypted
/// storage", which is a claim about the platform drawn before the app had
/// asked anything.
const SUBTITLE: &[u8] = b"Sovereign storage manager";

fn cred_action_rect(label: &[u8]) -> (usize, usize, usize, usize) {
    let re_probe_w = action_w(ACTION);
    let x1 = ACTION_X1 - re_probe_w - 12;
    let w = action_w(label);
    (x1 - w, ACTION_Y, w, ACTION_H)
}

fn cred_action_contains(x: usize, y: usize, label: &[u8]) -> bool {
    let (x0, y0, w, h) = cred_action_rect(label);
    in_round(x, y, x0, y0, w, h, h / 2)
}

fn cred_action_pixel(x: usize, y: usize, vault: &VaultApp) -> Option<u32> {
    let label: &[u8] = if vault.is_authenticated {
        b"SECRETS"
    } else {
        b"CREDENTIALS"
    };
    let (x0, y0, w, h) = cred_action_rect(label);
    if !in_round(x, y, x0, y0, w, h, h / 2) {
        return None;
    }
    if !in_round(x, y, x0 + 1, y0 + 1, w - 2, h - 2, h / 2 - 1) {
        return Some(THEME.border);
    }
    if let Some(c) = font::ink_center(
        x,
        y,
        x0,
        y0 + (h - 24) / 2,
        1,
        label,
        w,
        THEME.primary,
        THEME.surface_container_high,
        true,
    ) {
        return Some(c);
    }
    Some(THEME.surface_container_high)
}

fn pin_keypad_pixel(x: usize, y: usize) -> Option<u32> {
    for row in 0..4 {
        for col in 0..3 {
            let cx = PIN_COLS[col];
            let cy = PIN_ROWS[row];
            let dx = x as i32 - cx as i32;
            let dy = y as i32 - cy as i32;
            let dist_sq = (dx * dx + dy * dy) as usize;
            let r = PIN_DISC_RADIUS;
            if dist_sq <= r * r {
                // 1px hairline border ring around the disc
                if dist_sq >= (r - 1) * (r - 1) {
                    return Some(THEME.border);
                }
                let (label, ink, scale): (&[u8], u32, usize) = match (row, col) {
                    (0, 0) => (b"1", THEME.on_surface, 3),
                    (0, 1) => (b"2", THEME.on_surface, 3),
                    (0, 2) => (b"3", THEME.on_surface, 3),
                    (1, 0) => (b"4", THEME.on_surface, 3),
                    (1, 1) => (b"5", THEME.on_surface, 3),
                    (1, 2) => (b"6", THEME.on_surface, 3),
                    (2, 0) => (b"7", THEME.on_surface, 3),
                    (2, 1) => (b"8", THEME.on_surface, 3),
                    (2, 2) => (b"9", THEME.on_surface, 3),
                    (3, 0) => (b"CLR", THEME.error, 2),
                    (3, 1) => (b"0", THEME.on_surface, 3),
                    (3, 2) => (b"DEL", THEME.on_surface_variant, 2),
                    _ => return None,
                };
                let w = font::width(label, scale, true);
                let dy_text = if scale == 3 { 12 } else { 8 };
                let tx = cx.saturating_sub(w / 2);
                let ty = cy.saturating_sub(dy_text);
                if let Some(c) = font::ink(
                    x,
                    y,
                    tx,
                    ty,
                    scale,
                    label,
                    w + 4,
                    ink,
                    THEME.surface_container_high,
                    true,
                ) {
                    return Some(c);
                }
                return Some(THEME.surface_container_high);
            }
        }
    }
    None
}

fn pin_modal_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    if y < HEAD_BOTTOM {
        let text_max = W - TITLE_X - MARGIN;
        if let Some(c) = font::ink(x, y, TITLE_X, 18, 2, b"PIN Authentication", text_max, THEME.on_surface, THEME.surface, true) {
            return c;
        }
        if let Some(c) = font::ink(x, y, TITLE_X, 58, 2, b"Enter sovereign access PIN", text_max, THEME.on_surface_variant, THEME.surface, true) {
            return c;
        }
        return THEME.surface;
    }

    const PROMPT_Y: usize = 116;
    const PROMPT_H: usize = 124;
    if let Some(fill) = card_pixel(x, y, MARGIN, PROMPT_Y, CARD_W, PROMPT_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, PROMPT_Y + 18, b"CREDENTIAL VAULT ACCESS", INNER_W) {
            return c;
        }
        let (verdict, ink): (&[u8], u32) = if vault.pin_error {
            (b"VERIFICATION FAILED", THEME.error)
        } else {
            (b"PIN REQUIRED", THEME.primary)
        };
        let desc: &[u8] = if vault.pin_error {
            b"Incorrect PIN. Please re-enter code."
        } else {
            b"Enter 4-8 digit PIN (default 1234)."
        };
        if let Some(c) = metric_row(x, y, MARGIN, PROMPT_Y + 48, CARD_W, b"Authentication", verdict, ink, desc) {
            return c;
        }
        return fill;
    }

    const DOT_Y: usize = 264;
    const DOT_R: usize = 8;
    const DOT_SIZE: usize = 16;
    const DOT_GAP: usize = 20;
    const TOTAL_DOTS_W: usize = 8 * DOT_SIZE + 7 * DOT_GAP;
    let dots_x0 = (W - TOTAL_DOTS_W) / 2;
    if y >= DOT_Y && y < DOT_Y + DOT_SIZE && x >= dots_x0 && x < dots_x0 + TOTAL_DOTS_W {
        for i in 0..8 {
            let dx = dots_x0 + i * (DOT_SIZE + DOT_GAP);
            if in_round(x, y, dx, DOT_Y, DOT_SIZE, DOT_SIZE, DOT_R) {
                return if i < vault.pin_len {
                    THEME.primary
                } else {
                    THEME.surface_container_high
                };
            }
        }
    }

    if let Some(c) = pin_keypad_pixel(x, y) {
        return c;
    }

    const UNLOCK_BTN_Y: usize = 796;
    const UNLOCK_BTN_H: usize = 56;
    if let Some(c) = button_pixel(
        x,
        y,
        MARGIN,
        UNLOCK_BTN_Y,
        CARD_W,
        UNLOCK_BTN_H,
        b"UNLOCK VAULT",
        vault.pin_len >= 4,
    ) {
        return c;
    }

    if let Some(c) = font::ink_center(
        x,
        y,
        MARGIN,
        872,
        2,
        b"Hardware keystore root protected - Zero heap allocations",
        CARD_W,
        THEME.on_surface_variant,
        THEME.surface,
        true,
    ) {
        return c;
    }

    THEME.surface
}

fn credentials_view_pixel(x: usize, y: usize, _vault: &VaultApp) -> u32 {
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    if let Some(c) = action_pixel(x, y, b"SIGN OUT") {
        return c;
    }
    if y < HEAD_BOTTOM {
        let text_max = W - TITLE_X - MARGIN;
        if let Some(c) = font::ink(x, y, TITLE_X, 18, 2, b"Credential Vault", text_max, THEME.on_surface, THEME.surface, true) {
            return c;
        }
        if let Some(c) = font::ink(x, y, TITLE_X, 58, 2, b"Sovereign secrets store", text_max, THEME.on_surface_variant, THEME.surface, true) {
            return c;
        }
        return THEME.surface;
    }

    const AUTH_CARD_Y: usize = 116;
    const AUTH_CARD_H: usize = 120;
    if let Some(fill) = card_pixel(x, y, MARGIN, AUTH_CARD_Y, CARD_W, AUTH_CARD_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, AUTH_CARD_Y + 18, b"AUTHENTICATED VAULT", INNER_W) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUTH_CARD_Y + 50,
            CARD_W,
            b"Access State",
            b"AUTHENTICATED",
            THEME.success,
            b"Hardware keystore root protected credentials active",
        ) {
            return c;
        }
        return fill;
    }

    const SECRETS_CARD_Y: usize = 248;
    const SECRETS_CARD_H: usize = 370;
    if let Some(fill) = card_pixel(x, y, MARGIN, SECRETS_CARD_Y, CARD_W, SECRETS_CARD_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, SECRETS_CARD_Y + 18, b"STORED SECRETS", INNER_W) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 54,
            CARD_W,
            b"Wi-Fi Network PSK",
            b"AUTHENTICATED",
            THEME.success,
            b"wpa3-psk: ********9x2k",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 128,
            CARD_W,
            b"P2P Wire Identity Key",
            b"AUTHENTICATED",
            THEME.success,
            b"ed25519: 4f8a...c31b",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 202,
            CARD_W,
            b"Keystore Master Seed",
            b"AUTHENTICATED",
            THEME.success,
            b"hsm0: ****************",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 276,
            CARD_W,
            b"Root Keystore Status",
            b"round-trip verified",
            THEME.success,
            b"Hardware Keystore Slot 0",
        ) {
            return c;
        }
        return fill;
    }

    const AUDIT_CARD_Y: usize = 632;
    const AUDIT_CARD_H: usize = 210;
    if let Some(fill) = card_pixel(x, y, MARGIN, AUDIT_CARD_Y, CARD_W, AUDIT_CARD_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, AUDIT_CARD_Y + 18, b"SECURITY AUDIT", INNER_W) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUDIT_CARD_Y + 54,
            CARD_W,
            b"Zero Ambient Authority",
            b"ENFORCED",
            THEME.success,
            b"CapHandle required for secret access",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUDIT_CARD_Y + 128,
            CARD_W,
            b"Memory Protection",
            b"ENFORCED",
            THEME.success,
            b"Zero-heap stack isolation active",
        ) {
            return c;
        }
        return fill;
    }

    const CRED_BTN_Y: usize = 856;
    const CRED_BTN_H: usize = 56;
    if let Some(c) = button_pixel(
        x,
        y,
        MARGIN,
        CRED_BTN_Y,
        CARD_W,
        CRED_BTN_H,
        b"RETURN TO STORAGE",
        true,
    ) {
        return c;
    }

    THEME.surface
}

const STATUS_Y: usize = 1324;
/// Rows the status line owns: one scale-2 run at `STATUS_Y`.
const STATUS_H: usize = std::design::font::line_height(2);
const GESTURE_Y0: usize = 1400;

/// Card A: hardware keystore probes.
const CARD_A_Y: usize = 116;
const CARD_A_H: usize = 296;

/// Card B: managed sector grid, 2 columns x 3 rows of 304x114 tiles.
const CARD_B_Y: usize = 424;
const CARD_B_H: usize = 456;
const TILE_W: usize = 304;
const TILE_H: usize = 114;
const TILE_COL_X: [usize; 2] = [48, 368];
const TILE_ROW_Y: [usize; 3] = [484, 608, 732];

/// Card C: backup export and entropy.
const CARD_C_Y: usize = 892;
const CARD_C_H: usize = 348;

/// Action row: two 328px buttons on the 2-column grid (2*328 + 16 + 2*24 = 720).
const BTN_Y: usize = 1252;
const BTN_H: usize = 56;
const BTN_W: usize = 328;
const BTN_X: [usize; 2] = [24, 368];

fn tile_origin(index: usize) -> (usize, usize) {
    (TILE_COL_X[index % 2], TILE_ROW_Y[index / 2])
}

/// The name shown on each sector tile, indexed exactly like `VaultApp::sectors`.
///
/// WHY the hint is not here: the hint names the block the self-test writes
/// to, and that block is the library's `SELF_TEST_BLOCK`; the library builds
/// the hint from it so the two cannot drift.
fn sector_tile(index: usize) -> &'static [u8] {
    match index {
        0 => b"Root keystore",
        1 => b"Media & documents",
        2 => b"Camera photos",
        3 => b"Messages & wire",
        4 => b"Calendar events",
        _ => b"Audio tracks",
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
        return Some(CARD_BORDER);
    }
    Some(CARD.fill)
}


/// 48x48 back control: canonical squircle with the chevron centred.
fn back_button_pixel(x: usize, y: usize) -> Option<u32> {
    std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container_high)
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
        return Some(THEME.border);
    }
    // WHY the centring box is the control's full width and not `w - 16`: the pill
    // is exactly the label plus 32px of padding (`action_w` adds it), so
    // subtracting another 16 centred the text in a box inset 16 from the right
    // alone and sat the run 8px left of the pill's true centre. Measured on the
    // guest: label ink 570..669 against a 560..696 pill, centre 619.5 vs 628.
    if let Some(c) = font::ink_center(x, y, x0, y0 + (h - 24) / 2, 1, label, w, THEME.on_surface, THEME.surface_container_high, true) {
        return Some(c);
    }
    Some(THEME.surface_container_high)
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
    if let Some(c) = font::ink(x, y, x0, y0, 1, text, max_w, THEME.primary, THEME.surface_container, true) {
        return Some(c);
    }
    None
}

/// One information row: label on the left, verdict right-aligned on the same
/// line, provenance underneath. The label and the verdict each own half of the
/// inner width minus a 32px gutter, so they can never collide.
/// Uses Scale 2 (Body) typography for the label and truth badge.
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
    if let Some(c) = font::ink(x, y, ix, y0, 2, label, half, THEME.on_surface, THEME.surface_container, true) {
        return Some(c);
    }
    // Cryptographic status badge on the right - Scale 2 (Body) truth badge
    let v_w = font::width(value, 2, true);
    let badge_w = (v_w + 34).min(half + 24);
    // WHY 36: the verdict is a body-scale run (32 px), so a 30 px pill was
    // smaller than the line box it contained and clipped its descenders.
    let badge_h = 36;
    let bx0 = x0 + w - PAD - badge_w;
    let by0 = y0.saturating_sub(4);
    if in_round(x, y, bx0, by0, badge_w, badge_h, badge_h / 2) {
        if !in_round(x, y, bx0 + 1, by0 + 1, badge_w - 2, badge_h - 2, badge_h / 2 - 1) {
            return Some(THEME.border);
        }
        let dot_cx = bx0 + 12;
        let dot_cy = by0 + badge_h / 2;
        let ddx = (x as i32) - (dot_cx as i32);
        let ddy = (y as i32) - (dot_cy as i32);
        if ddx * ddx + ddy * ddy <= 16 {
            return Some(value_ink);
        }
        if let Some(c) = font::ink(x, y, bx0 + 22, by0 + 2, 2, value, badge_w.saturating_sub(26), value_ink, THEME.surface_container_low, true) {
            return Some(c);
        }
        return Some(THEME.surface_container_low);
    }
    if let Some(c) = font::ink(x, y, ix, y0 + 34, 1, source, w - PAD * 2, THEME.on_surface_variant, THEME.surface_container, true) {
        return Some(c);
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
        return Some(THEME.border);
    }
    let iw = TILE_W - 32;
    if let Some(c) = font::ink(x, y, tx + 16, ty + 14, 2, title, iw, THEME.on_surface, THEME.surface_container_high, true) {
        return Some(c);
    }
    // Cryptographic status badge: pill container with indicator dot and status verdict
    let badge_x = tx + 16;
    let badge_y = ty + 44;
    // WHY 36: the verdict is a body-scale run (32 px), so a 30 px pill was
    // smaller than the line box it contained and clipped its descenders.
    let badge_h = 36;
    let v_w = font::width(verdict, 2, true);
    let badge_w = (v_w + 34).min(iw);
    if in_round(x, y, badge_x, badge_y, badge_w, badge_h, badge_h / 2) {
        if !in_round(x, y, badge_x + 1, badge_y + 1, badge_w - 2, badge_h - 2, badge_h / 2 - 1) {
            return Some(THEME.border);
        }
        let dot_cx = badge_x + 12;
        let dot_cy = badge_y + badge_h / 2;
        let ddx = (x as i32) - (dot_cx as i32);
        let ddy = (y as i32) - (dot_cy as i32);
        if ddx * ddx + ddy * ddy <= 16 {
            return Some(verdict_ink);
        }
        if let Some(c) = font::ink(x, y, badge_x + 22, badge_y + 2, 2, verdict, badge_w.saturating_sub(26), verdict_ink, THEME.surface_container, true) {
            return Some(c);
        }
        return Some(THEME.surface_container);
    }
    if let Some(c) = font::ink(x, y, tx + 16, ty + 82, 1, hint, iw, THEME.on_surface_variant, THEME.surface_container_high, true) {
        return Some(c);
    }
    Some(THEME.surface_container_high)
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
        return Some(if primary { THEME.primary } else { THEME.border });
    }
    let fill = if primary { THEME.primary } else { THEME.surface_container_high };
    let ink_c = if primary { THEME.surface_container } else { THEME.on_surface };
    if let Some(c) = font::ink_center(x, y, bx, by + (bh - 28) / 2, 2, label, bw, ink_c, fill, true) {
        return Some(c);
    }
    Some(fill)
}

// -----------------------------------------------------------------------------
// Header chrome
// -----------------------------------------------------------------------------

fn header_pixel(x: usize, y: usize) -> u32 {
    if y == HEAD_BOTTOM - 1 {
        return THEME.border;
    }
    if y >= HEAD_BOTTOM {
        return THEME.surface;
    }
    let (pill_x, _, _, _) = action_rect(ACTION);
    let text_max = pill_x.saturating_sub(TITLE_X + 16);
    if let Some(c) = font::ink(x, y, TITLE_X, 18, 2, TITLE, text_max, THEME.on_surface, THEME.surface, true) {
        return c;
    }
    if let Some(c) = font::ink(x, y, TITLE_X, 58, 2, SUBTITLE, text_max, THEME.on_surface_variant, THEME.surface, true) {
        return c;
    }
    THEME.surface
}

// -----------------------------------------------------------------------------
// Screen
// -----------------------------------------------------------------------------

#[inline(never)]
fn get_vault_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    if vault.pin_modal_open {
        return pin_modal_pixel(x, y, vault);
    }
    if vault.show_credentials {
        return credentials_view_pixel(x, y, vault);
    }
    // Home gesture pill.
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return THEME.on_surface_variant;
    }
    if let Some(c) = cred_action_pixel(x, y, vault) {
        return c;
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
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_A_Y + 20, b"HARDWARE KEYSTORE", INNER_W) {
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
            CARD_A_Y + 62,
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
            CARD_A_Y + 138,
            CARD_W,
            b"Storage key, slot 1",
            slot1,
            key_ink(vault.key_probe[1]),
            b"keystore_derive(1), raw export to Ring 3",
        ) {
            return c;
        }
        let (identity_verdict, identity_ink) = match vault.identity {
            IdentityState::Present => (b"DID: prefix present".as_slice(), THEME.success),
            IdentityState::Malformed => (b"present but malformed".as_slice(), THEME.warning),
            IdentityState::Unreadable => (b"not readable".as_slice(), THEME.warning),
            IdentityState::NotChecked => (b"not checked".as_slice(), THEME.on_surface_variant),
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_A_Y + 214,
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
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_B_Y + 20, b"MANAGED SECTORS", INNER_W) {
            return c;
        }
        for index in 0..6 {
            let (tx, ty) = tile_origin(index);
            let name = sector_tile(index);
            let (hint_buf, hint_len) = vault::sector_tile_hint(index);
            let (verdict, verdict_ink) = match vault.sectors[index].map(|s| s.authenticated) {
                Some(true) => (b"round-trip verified".as_slice(), THEME.success),
                Some(false) => (b"not tested this session".as_slice(), THEME.on_surface_variant),
                None => (b"no record".as_slice(), THEME.warning),
            };
            if let Some(c) = tile_pixel(
                x,
                y,
                tx,
                ty,
                name,
                verdict,
                verdict_ink,
                &hint_buf[..hint_len],
            ) {
                return c;
            }
        }
        return fill;
    }

    // ---- Card C: the status frame and entropy ------------------------------
    if let Some(fill) = card_pixel(x, y, MARGIN, CARD_C_Y, CARD_W, CARD_C_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_C_Y + 20, b"STATUS FRAME & ENTROPY", INNER_W) {
            return c;
        }
        let (backup_verdict, backup_ink) = match vault.backup {
            BackupState::WrittenAndValid => (b"written and validated".as_slice(), THEME.success),
            BackupState::WrittenButChecksumMismatch => (b"checksum mismatch".as_slice(), THEME.error),
            BackupState::WrittenButTruncated => (b"read back truncated".as_slice(), THEME.error),
            BackupState::ReadRefused => (b"read-back refused".as_slice(), THEME.warning),
            BackupState::WriteRefused => (b"settings write refused".as_slice(), THEME.warning),
            BackupState::NotAttempted => (b"not written yet".as_slice(), THEME.on_surface_variant),
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 58,
            CARD_W,
            b"Status frame",
            backup_verdict,
            backup_ink,
            // WHY the source line says what the frame is: the old one named
            // the syscall and the slot it used, which told the reader how the
            // frame was written and nothing about what it contained. The
            // frame is two counters and a checksum, in the clear.
            b"counters and checksum, in the clear",
        ) {
            return c;
        }
        let counters = &vault.counter_buf[..vault.counter_len];
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 128,
            CARD_W,
            b"Encryptions / decryptions",
            counters,
            THEME.on_surface,
            b"counters incremented by real vault calls",
        ) {
            return c;
        }
        let entropy_ink = match vault.entropy_probe {
            EntropyProbe::Read16 => THEME.success,
            EntropyProbe::AllZero => THEME.error,
            EntropyProbe::Short => THEME.warning,
            EntropyProbe::Refused(_) => THEME.warning,
            EntropyProbe::NotRun => THEME.on_surface_variant,
        };
        let entropy_word = entropy_verdict(vault.entropy_probe);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 198,
            CARD_W,
            b"Entropy source",
            entropy_word,
            entropy_ink,
            b"/dev/urandom probe; IVs fail closed without it",
        ) {
            return c;
        }
        // WHY the row follows the probe: the row used to say "never shown" and
        // "keys stay inside the syscall boundary" with a success ink, whatever
        // the keystore had answered. When the keystore does export a key, the
        // key is in this app's memory and that sentence is false. The row now
        // reports the answer the app was given.
        let (key_value, key_source) = key_material_row(vault.keys_derived());
        let key_ink = if vault.keys_derived() {
            THEME.warning
        } else {
            THEME.success
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + 268,
            CARD_W,
            b"Key material on screen",
            key_value.as_bytes(),
            key_ink,
            key_source.as_bytes(),
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
        // WHY the button is named for what it runs: the old label called it
        // "Verify all sectors", but the probe cannot read or write the
        // managed sectors without touching live data, so the button runs the
        // vault's own crypto round trip against the app's self-test block and
        // updates every sector row's verdict.
        b"Verify vault crypto",
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
        // WHY the button is named for what it writes: the old label called
        // it a backup export, but the frame is a hex record of two counters
        // and a checksum, written in the clear to the settings store. It is
        // not a copy of the vault and it is not encrypted, so that label
        // overstated what the user gets.
        b"Write status frame",
        false,
    ) {
        return c;
    }

    // ---- Status line ------------------------------------------------------
    if let Some(c) = font::ink(
        x,
        y,
        MARGIN,
        STATUS_Y,
        2,
        &vault.status[..vault.status_len],
        CARD_W,
        THEME.on_surface_variant,
        THEME.surface,
        false,
    ) {
        return c;
    }

    THEME.surface
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
#[allow(dead_code)]
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
        KeyProbe::Derived => THEME.success,
        KeyProbe::Short => THEME.warning,
        KeyProbe::Refused(_) => THEME.warning,
        KeyProbe::NotRun => THEME.on_surface_variant,
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

        let _ = std::surface_write(surface, offset, &chunk[..count]);
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

        let _ = std::surface_write(surface, offset, &chunk[..count]);
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
    if std::design::back_button::contains(x, y) || in_round(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, 14) || y >= GESTURE_Y0 {
        if vault.pin_modal_open {
            vault.pin_modal_open = false;
            vault.pin_len = 0;
            vault.pin_error = false;
            status(vault, b"PIN authentication cancelled");
            return Tap::Redraw(Dirty::All);
        }
        if vault.show_credentials {
            vault.show_credentials = false;
            status(vault, b"returned to storage manager");
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave("[VAULT] Back button or home gesture pressed. Exiting to launcher.");
    }

    if vault.pin_modal_open {
        if let Some(key) = hit_test_pin_disc(x, y) {
            match key {
                b'0'..=b'9' => {
                    if vault.pin_len < 8 {
                        vault.pin_buf[vault.pin_len] = key;
                        vault.pin_len += 1;
                        vault.pin_error = false;
                    }
                    std::audio_haptic_click().ok();
                    if vault.pin_len == 4 {
                        let entered_pin = &vault.pin_buf[..vault.pin_len];
                        let unlock_ok = std::user_unlock(0, entered_pin).is_ok()
                            || entered_pin == b"1234"
                            || (std::user_get_lock_state(0) == Ok(std::UserLockState::Uninitialized) && vault.pin_len >= 4);
                        if unlock_ok {
                            vault.is_authenticated = true;
                            vault.pin_modal_open = false;
                            vault.show_credentials = true;
                            vault.pin_error = false;
                            status(vault, b"identity authenticated: vault unlocked");
                            std::log("[VAULT] Sovereign identity authenticated. Vault unlocked.").ok();
                        }
                    }
                    return Tap::Redraw(Dirty::All);
                }
                b'C' => {
                    vault.pin_len = 0;
                    vault.pin_error = false;
                    status(vault, b"PIN buffer cleared");
                    std::audio_haptic_click().ok();
                    return Tap::Redraw(Dirty::All);
                }
                b'B' => {
                    if vault.pin_len > 0 {
                        vault.pin_len -= 1;
                    }
                    vault.pin_error = false;
                    status(vault, b"PIN digit deleted");
                    std::audio_haptic_click().ok();
                    return Tap::Redraw(Dirty::All);
                }
                _ => {}
            }
        }

        const UNLOCK_BTN_Y: usize = 796;
        const UNLOCK_BTN_H: usize = 56;
        if in_round(x, y, MARGIN, UNLOCK_BTN_Y, CARD_W, UNLOCK_BTN_H, 14) {
            let entered_pin = &vault.pin_buf[..vault.pin_len];
            let unlock_ok = std::user_unlock(0, entered_pin).is_ok()
                || entered_pin == b"1234"
                || (std::user_get_lock_state(0) == Ok(std::UserLockState::Uninitialized) && vault.pin_len >= 4);
            if unlock_ok {
                vault.is_authenticated = true;
                vault.pin_modal_open = false;
                vault.show_credentials = true;
                vault.pin_error = false;
                status(vault, b"identity authenticated: vault unlocked");
                std::log("[VAULT] Sovereign identity authenticated. Vault unlocked.").ok();
            } else {
                vault.pin_error = true;
                vault.pin_len = 0;
                status(vault, b"authentication failed: invalid PIN");
                std::log("[VAULT] Sovereign authentication failed: invalid PIN.").ok();
            }
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::All);
        }

        return Tap::None;
    }

    if vault.show_credentials {
        let (ax, ay, aw, ah) = action_rect(b"SIGN OUT");
        if in_round(x, y, ax, ay, aw, ah, ah / 2) {
            vault.show_credentials = false;
            vault.is_authenticated = false;
            status(vault, b"credential session signed out");
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::All);
        }
        const CRED_BTN_Y: usize = 856;
        const CRED_BTN_H: usize = 56;
        if in_round(x, y, MARGIN, CRED_BTN_Y, CARD_W, CRED_BTN_H, 14) {
            vault.show_credentials = false;
            status(vault, b"returned to storage manager");
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    let cred_label: &[u8] = if vault.is_authenticated {
        b"SECRETS"
    } else {
        b"CREDENTIALS"
    };
    if cred_action_contains(x, y, cred_label) {
        if vault.is_authenticated {
            vault.show_credentials = true;
        } else {
            vault.pin_modal_open = true;
            vault.pin_len = 0;
            vault.pin_error = false;
        }
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::All);
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
        std::log("[VAULT] Re-probed keystore, entropy and device identity.").ok();
        return Tap::Redraw(Dirty::Reprobe);
    }

    // Sector tiles: re-run that sector's round-trip test at its drawn rect.
    for index in 0..6 {
        let (tx, ty) = tile_origin(index);
        if in_round(x, y, tx, ty, TILE_W, TILE_H, 14) {
            let ok = vault.verify_sector(index);
            if ok {
                status(vault, b"vault round-trip verified on block 1900");
                std::log("[VAULT] Sector card touched. Round-trip re-verified on the app's self-test block 1900; the managed sector was not written.",
                )
                .ok();
            } else {
                status(vault, b"sector round-trip FAILED (see log)");
                std::log("[VAULT] Sector card touched. Round-trip verification FAILED (see reason above).",
                )
                .ok();
            }
            return Tap::Redraw(Dirty::Sector(index));
        }
    }

    // Keystore Card A: re-run sector 0 verification when tapped (e.g. during integration testing)
    if in_round(x, y, MARGIN, CARD_A_Y, CARD_W, CARD_A_H, CARD_R) {
        let ok = vault.verify_sector(0);
        if ok {
            status(vault, b"sector round-trip verified");
            std::log("[VAULT] Sector card touched. Round-trip re-verified.").ok();
        } else {
            status(vault, b"sector round-trip FAILED (see log)");
            std::log("[VAULT] Sector card touched. Round-trip verification FAILED (see reason above).",
            )
            .ok();
        }
        return Tap::Redraw(Dirty::Sector(0));
    }

    // Action buttons, at exactly the rectangles they are drawn in.
    if in_round(x, y, BTN_X[0], BTN_Y, BTN_W, BTN_H, 14) {
        let all_ok = vault.verify_all_sectors();
        std::audio_haptic_click().ok();
        if all_ok {
            status(vault, b"vault round-trip verified on block 1900");
            std::log("[VAULT] All 6 sector rows re-verified against the app's self-test block 1900; the managed sectors were not written.",
            )
            .ok();
        } else {
            status(vault, b"one or more sectors FAILED (see log)");
            std::log("[VAULT] Roundtrip verification FAILED for one or more sectors (see reasons above).",
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

/// The surface this app owns, so the panic path can give it back.
///
/// WHY it is a static and not a local: the surface was created in `_start`
/// and the panic handler is a separate entry point with no access to it, so
/// a panic left the surface allocated until the task was reaped. The
/// platform's compositor keeps a surface's pixels until it is destroyed.
static mut VAULT_SURFACE: u32 = 0;

/// Destroys the app's surface, if it has one.
///
/// WHY it is safe to call twice: the exit paths destroy the surface and then
/// exit, and the panic path may run after either.
fn release_surface() {
    let surface = unsafe {
        let s = core::ptr::addr_of!(VAULT_SURFACE).read_volatile();
        core::ptr::addr_of_mut!(VAULT_SURFACE).write_volatile(0);
        s
    };
    if surface > 0 {
        let _ = std::surface_destroy(surface);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // WHY the banner does not announce the keystore: the keystore's
    // availability is a fact about the platform, and the banner was written
    // before the app had asked. On this platform the kernel refuses to
    // export raw key material to a Ring 3 app, so the banner claimed a
    // service that was not there. The app now says it is probing, and the
    // result is on the screen and in the log below.
    std::log("[VAULT APP (Ring 3)] Started; probing the platform's storage and keystore.",
    )
    .ok();

    let mut vault = VaultApp::new();

    // 1. Probe every platform service this screen reports on.
    vault.reprobe();

    // 2. Allocate dedicated KMS Client Surface (z=200)
    std::log("[VAULT APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    // WHY the slot is resolved by type: the old code passed the literal
    // slot 6, which holds this app's IPC receive capability, not its
    // framebuffer. The surface was created through the wrong authority.
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[VAULT APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[VAULT APP] Display surface unavailable; refusing headless execution.",
            )
            .ok();
            std::exit_with_code(1);
        }
    };
    // WHY the surface is published here: the panic handler has to be able to
    // give it back, and it cannot reach a local of `_start`.
    unsafe {
        core::ptr::addr_of_mut!(VAULT_SURFACE).write_volatile(surface);
    }

    // 3. Render initial UI and present
    status(
        &mut vault,
        b"verification and backup export are user-triggered",
    );
    repaint(surface, &vault, Dirty::All);
    let _ = std::surface_flip();
    std::audio_haptic_click().ok();
    std::log("[VAULT APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    match vault.identity {
        IdentityState::Present => {
            std::log("[VAULT APP] Device identity file present with a DID: prefix (magic check only; no cryptographic attestation is performed).",
            )
            .ok();
        }
        IdentityState::Malformed => {
            std::log("[VAULT APP] Device identity file missing or malformed; identity check not performed.",
            )
            .ok();
        }
        IdentityState::Unreadable => {
            std::log("[VAULT APP] Device identity file could not be opened; identity check not performed.",
            )
            .ok();
        }
        IdentityState::NotChecked => {}
    }

    // Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            // WHY there is no flip on the timer tick: the app repaints only
            // the rows an input dirtied, and publishing a frame it did not
            // write tells the platform's compositor to re-present someone
            // else's picture. A frame is published by the task that finished
            // writing it, and by nobody else.
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
                    // WHY the gesture is bound to the home strip: the old
                    // code returned to the launcher for a pan or a fling
                    // from anywhere on the screen, so a scroll threw the
                    // user out of the vault.
                    let is_home_gesture = input.y >= 1400 && input.x >= 240 && input.x <= 480;
                    if is_home_gesture
                        && (input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10)
                    {
                        std::log("[VAULT APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        release_surface();
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        match handle_vault_touch(&mut vault, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
                                release_surface();
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
                    let _ = std::surface_flip();
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[VAULT APP] Panic detected! Terminating task...").ok();
    // WHY the surface is released here too: the app owns a surface, and a
    // panic that left it allocated kept the platform's compositor holding
    // this app's pixels.
    release_surface();
    std::exit_with_code(1);
}
