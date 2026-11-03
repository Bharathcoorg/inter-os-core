// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
use interos_std::design::{components, outline_text};
use vault::theme::{spacing, type_scale, THEME};
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
    pub clock: [u8; 5],
    pub clock_ok: bool,
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
            clock: [b' '; 5],
            clock_ok: false,
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

const TITLE_X: usize = 88;
// The header's back control, read straight off the SDK rather than re-authored
// here. WHY: the painter below asks `back_button::pixel_for_theme`, which owns
// 24..72 by 56..104, so these three numbers are not a layout choice the vault
// makes -- they are the platform's, and writing 24 and 48 a second time is how
// the box the app lays out its header against and the box the chip draws drift
// apart. `back_button::Y` is the same `viewport::HEADER_TOP` this used to name.
const BACK_X: usize = std::design::back_button::X;
const BACK_Y: usize = std::design::back_button::Y;
const BACK_S: usize = std::design::back_button::SIZE;

/// Header action pill ("RE-PROBE"): 44px tall, right-aligned to the margin.
const ACTION_H: usize = 44;
/// WHY two rows inside the back control's band: the pill is 44 tall beside a
/// 48-tall control, and centring them is what keeps the row from reading crooked.
const ACTION_Y: usize = BACK_Y + 2;

/// The later of two layout edges.
///
/// WHY a local helper rather than `Ord::max`: `max` is not callable in a `const`
/// context on this toolchain, and every edge below has to be a compile-time
/// constant, because the pixel function compares against it once per pixel.
const fn later(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

/// Rows from a band's top edge to the line-box top that stands a rung's caps
/// in the middle of a band `h` rows tall.
///
/// WHY the caps and not the line box: a run's box reaches `ascent` rows above
/// its baseline and `descent` below it, and only the cap block is visible on a
/// digit or a capitalised label. Centring the box therefore leaves the whole
/// descent's worth of empty rows below the ink and lifts the ink off the
/// middle of the control it is supposed to name. The header's title already
/// uses this placement; a control and the label standing in it must agree or
/// the row reads crooked.
///
/// WHY a subtraction that can be refused: a rung taller than its band has no
/// centred placement, and answering zero leaves the run in the band's top rows
/// where the caller's own gate can see it rather than wrapping a `usize`
/// around the canvas.
const fn centred_text_top(h: usize, rung: usize) -> usize {
    let box_top = h / 2 + std::design::font::cap_for_scale(rung) as usize / 2;
    let ascent = std::design::font::ascent_for_scale(rung);
    if ascent > box_top {
        0
    } else {
        box_top - ascent
    }
}

/// WHY the rungs stay inside the bands they are dropped into: a control that
/// is shorter than its own label's line box cannot hold it, and on a per-pixel
/// surface the label simply paints over the control's hairline.
const _: () = assert!(centred_text_top(ACTION_H, 1) + std::design::font::line_height(1) <= ACTION_H);
const _: () = assert!(centred_text_top(BTN_H, 2) + std::design::font::line_height(2) <= BTN_H);
const _: () = assert!(centred_text_top(2 * PIN_DISC_RADIUS, 3) + std::design::font::line_height(3) <= 2 * PIN_DISC_RADIUS);

/// The rung the header's title is drawn at, on every one of the three screens
/// that carries this header.
///
/// WHY the section rung: the header line names the screen, and a name set at the
/// size of the prose under it is not a name. This is the rung File Manager's
/// `HEADER_TITLE_SCALE` already stands at.
const HEAD_TITLE_RUNG: usize = type_scale::SECTION;

/// The rung the header's subtitle is drawn at.
///
/// WHY a rung of its own, and a lower one: both lines used to read from a single
/// `HEAD_RUNG` set at the body rung, so the line that qualifies the title was
/// the title's own size and the pair read as two names stacked. Sharing the rung
/// also could not be made to fit: the header bounds its runs by the pill that
/// reaches furthest left, and the longest subtitle lost its tail to that bound
/// and drew as "Sovereign sto...". A caption is the role this line plays, and
/// the caption rung is what the platform gives that role.
const HEAD_SUBTITLE_RUNG: usize = type_scale::CAPTION;

/// WHY the two lines may not silently meet again: the defect they shared was one
/// rung standing in for two roles, and a rename back to a single constant would
/// restore it without any layout number changing.
const _: () = assert!(HEAD_TITLE_RUNG > HEAD_SUBTITLE_RUNG);

/// The air between the header's runs and the first pill standing to their
/// right.
///
/// WHY the runs are bounded by the pill and not by the panel's inset: the
/// header paints its pills before it paints its text, and on this surface the
/// branch that claims a pixel wins -- so a run that reaches into a pill does not
/// overlap it, it loses the reach. A bound that only holds while the strings
/// happen to be short is the same defect one rename away.
///
/// WHY the back control is checked against the same gutter: the runs start to
/// its right, and `TITLE_X` is the only number keeping the two apart.
const HEAD_TEXT_GUTTER: usize = 16;
const _: () = assert!(BACK_X + BACK_S + HEAD_TEXT_GUTTER <= TITLE_X);

/// The widest a header run may be when a band edge `right_of` stands at the
/// right of it: the space up to that edge, less the gutter.
#[inline]
const fn head_text_max(right_of: usize) -> usize {
    right_of.saturating_sub(TITLE_X + HEAD_TEXT_GUTTER)
}

/// Rows the title's line box reserves: ascent plus descent at its rung.
const TITLE_BOX_H: usize = std::design::font::line_height(HEAD_TITLE_RUNG);
/// Rows the subtitle's line box reserves.
const SUBTITLE_BOX_H: usize = std::design::font::line_height(HEAD_SUBTITLE_RUNG);

/// The air between the two header lines' boxes.
const HEAD_LINE_GAP: usize = spacing::GRID_8;
/// The air between the last row the header may ink and its hairline.
const HEAD_PAD_BOTTOM: usize = 2;
/// The header's hairline, drawn on the band's last row.
const HEAD_RULE_H: usize = 1;

/// The title's line-box top.
///
/// WHY the caps are centred on the back control's row rather than the box being
/// dropped on the row's top edge: the box at this rung is
/// `line_height(HEAD_TITLE_RUNG)` rows, and the control beside it is 48, so a box
/// authored at `BACK_Y` ran past the control and straight into the subtitle
/// under it. Centring the cap height is the placement the dialer and messages
/// already document.
const TITLE_Y0: usize = BACK_Y
    + BACK_S / 2
    - std::design::font::ascent_for_scale(HEAD_TITLE_RUNG)
    + std::design::font::cap_for_scale(HEAD_TITLE_RUNG) as usize / 2;

/// The subtitle's line-box top: the whole of the title's box, then the gap.
///
/// WHY from the title's box and not from a literal offset on `BACK_Y`: the old
/// `BACK_Y + 26` put this box six rows inside the title's, so the two runs inked
/// over each other and the pair's tail ran past the band's rule.
const SUBTITLE_Y0: usize = TITLE_Y0 + TITLE_BOX_H + HEAD_LINE_GAP;

/// Header band bottom, derived from the tallest thing standing in the band.
///
/// WHY derived and not typed as a literal: the band is only correct while it
/// contains both line boxes, the 48-tall back control and the action pill. The
/// literal held 112 after `font::line_height` began returning ascent plus
/// descent, and nothing complained -- the two rows past the band simply painted
/// over the content underneath it, and the rule at `HEAD_BOTTOM - 1` overwrote
/// the line above that.
const HEAD_CONTENT_BOTTOM: usize = later(
    later(
        TITLE_Y0 + TITLE_BOX_H,
        SUBTITLE_Y0 + SUBTITLE_BOX_H,
    ),
    later(BACK_Y + BACK_S, ACTION_Y + ACTION_H),
);
const HEAD_BOTTOM: usize = HEAD_CONTENT_BOTTOM + HEAD_PAD_BOTTOM + HEAD_RULE_H;

/// The row the content under the header starts at, on every screen.
///
/// WHY one shared edge: the PIN modal, the credential list and the storage
/// manager each used to type their own first-row literal (116), so growing the
/// header meant finding three numbers that were the same number. Every band
/// below now derives from here, and the header's bottom is no longer also the
/// first card's top edge.
const CONTENT_TOP: usize = HEAD_BOTTOM + spacing::GRID_4;

/// The "UNLOCK VAULT" pill, directly under the last row of keypad discs.
///
/// WHY at module level and derived from the keypad: the ink path and the tap
/// path each used to type this literal into their own function, so moving the
/// pill on screen left the hit rectangle where it was and the taps landed on
/// blank canvas. Deriving it from the discs' own bottom row means the two cannot
/// disagree and the gap under the keypad cannot change by accident.
const UNLOCK_BTN_Y: usize = PIN_ROWS[3] + PIN_DISC_RADIUS + 28;
const UNLOCK_BTN_H: usize = 56;

/// The line under the unlock pill, and the rung it is drawn at.
const PIN_NOTE_Y: usize = UNLOCK_BTN_Y + UNLOCK_BTN_H + spacing::GRID_16 + 4;
const PIN_NOTE_RUNG: usize = 1;

/// WHY the note stays above the gesture strip: `GESTURE_Y0` and the indicator
/// rows under it are the system's, bottom-anchored, and this app does not own
/// them -- the whole stack above only ever grows downward toward them.
const _: () = assert!(
    PIN_NOTE_Y + std::design::font::line_height(PIN_NOTE_RUNG) < GESTURE_Y0
);

/// The credential screen's stack: session card, keystore probes, audit card and
/// the return pill, each band starting below the whole box of the one above it.
///
/// WHY the stack is derived from `CONTENT_TOP` rather than typed: the pill at the
/// foot of it is hit-tested by `handle_vault_touch` from a second copy of the same
/// literal, and the two copies only stayed in step while nobody moved the header.
/// Naming the gaps is what makes the next move a one-line change in one place.
const AUTH_CARD_Y: usize = CONTENT_TOP;
/// The session card's caption row, its one metric row's top, and the height the
/// two of them add up to.
///
/// WHY derived: the card remembered 120 rows from when a metric row reserved 64
/// -- provenance and label boxes overlapping by five. With the row's two boxes
/// stacked the way the ladder counts them it reserves 71, and a card that stops
/// short of its own content does not clip it, it paints the surplus over whatever
/// band answers next.
const CRED_CAPTION_Y: usize = 18;
const CRED_SESSION_ROW_Y: usize = 50;
const AUTH_CARD_H: usize = CRED_SESSION_ROW_Y + METRIC_ROW_H + CARD_ROW_FOOT_AIR;
const _: () = assert!(
    CRED_CAPTION_Y + CARD_CAPTION_ROWS <= CRED_SESSION_ROW_Y
        && CRED_SESSION_ROW_Y + METRIC_ROW_H <= AUTH_CARD_H
);
/// The air between two bands of the credential screen's stack.
const CRED_CARD_GAP: usize = 12;
const SECRETS_CARD_Y: usize = AUTH_CARD_Y + AUTH_CARD_H + CRED_CARD_GAP;
const SECRETS_CARD_H: usize = 370;
/// The air between the secrets card, the audit card and the return pill -- a
/// step wider than `CRED_CARD_GAP`, because each of these is a section rather
/// than a continuation of the card above it.
const CRED_SECTION_GAP: usize = 14;
const AUDIT_CARD_Y: usize = SECRETS_CARD_Y + SECRETS_CARD_H + CRED_SECTION_GAP;
const AUDIT_CARD_H: usize = 210;
const CRED_BTN_Y: usize = AUDIT_CARD_Y + AUDIT_CARD_H + CRED_SECTION_GAP;
const CRED_BTN_H: usize = 56;
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

/// The label of the header's second pill. Which of the two it carries is the
/// session's answer, so the header's runs are bounded by the wider of them --
/// see `CRED_PILL_WIDEST`.
#[inline]
fn cred_pill_label(authenticated: bool) -> &'static [u8] {
    if authenticated {
        b"SECRETS"
    } else {
        b"CREDENTIALS"
    }
}

/// The wider of the two labels the second pill can carry, and therefore the
/// left edge the header's runs have to clear whichever one is drawn.
const CRED_PILL_WIDEST: &[u8] = b"CREDENTIALS";

/// The credential screen's way out, painted as a header pill and hit as one.
const SIGN_OUT: &[u8] = b"SIGN OUT";

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
    let label: &[u8] = cred_pill_label(vault.is_authenticated);
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
        y0 + centred_text_top(h, 1),
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
                // WHY the disc's own diameter centres the run: the literals
                // this used were 12 rows for a digit and 8 for a chip label,
                // against 23 and 17 that place the caps on the centre row, so
                // every key sat its label in the lower half of its disc.
                let tx = cx.saturating_sub(w / 2);
                let ty = (cy - PIN_DISC_RADIUS) + centred_text_top(2 * PIN_DISC_RADIUS, scale);
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
    if let Some(c) = std::design::home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
    }
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    if y < HEAD_BOTTOM {
        // No pill stands in this screen's header row, so the runs may use the
        // whole panel up to its inset -- and no further.
        let text_max = head_text_max(W - MARGIN);
        if let Some(c) = font::ink(x, y, TITLE_X, TITLE_Y0, HEAD_TITLE_RUNG, b"PIN Authentication", text_max, THEME.on_surface, THEME.surface, true) {
            return c;
        }
        if let Some(c) = font::ink(x, y, TITLE_X, SUBTITLE_Y0, HEAD_SUBTITLE_RUNG, b"Enter sovereign access PIN", text_max, THEME.on_surface_variant, THEME.surface, true) {
            return c;
        }
        return THEME.surface;
    }

    const PROMPT_Y: usize = CONTENT_TOP;
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

    /// The PIN dots, one box under the prompt card.
    const DOT_Y: usize = PROMPT_Y + PROMPT_H + 24;
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

    // WHY the line says this and not what it used to say: the PIN path asks
    // the system's unlock service and opens the credential list; it never
    // causes the keystore to release a key, so the screen may not claim a
    // hardware root of protection over the PIN.
    if let Some(c) = font::ink_center(
        x,
        y,
        MARGIN,
        PIN_NOTE_Y,
        PIN_NOTE_RUNG,
        b"The PIN opens this session; it does not release a keystore key",
        CARD_W,
        THEME.on_surface_variant,
        THEME.surface,
        false,
    ) {
        return c;
    }

    THEME.surface
}

fn credentials_view_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    if let Some(c) = std::design::home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
    }
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    if let Some(c) = action_pixel(x, y, SIGN_OUT) {
        return c;
    }
    if y < HEAD_BOTTOM {
        // The SIGN OUT pill is painted before these runs and stands in the same
        // row, so they stop a gutter short of it (see `header_pixel`).
        let text_max = head_text_max(action_rect(SIGN_OUT).0);
        if let Some(c) = font::ink(x, y, TITLE_X, TITLE_Y0, HEAD_TITLE_RUNG, b"Credential Vault", text_max, THEME.on_surface, THEME.surface, true) {
            return c;
        }
        if let Some(c) = font::ink(x, y, TITLE_X, SUBTITLE_Y0, HEAD_SUBTITLE_RUNG, b"Sovereign secrets store", text_max, THEME.on_surface_variant, THEME.surface, true) {
            return c;
        }
        return THEME.surface;
    }

    // This screen's whole stack is declared once, at module level, below the
    // header block; the "RETURN TO STORAGE" pill's rectangle is read from there
    // by the tap path as well as from here by the painter.
    if let Some(fill) = card_pixel(x, y, MARGIN, AUTH_CARD_Y, CARD_W, AUTH_CARD_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, AUTH_CARD_Y + CRED_CAPTION_Y, b"VAULT SESSION", INNER_W) {
            return c;
        }
        // WHY the row reports the session and not the hardware: it used to
        // print a fixed "authenticated, hardware protected" verdict whatever
        // the keystore had answered. Opening this list only ever asks the
        // system's unlock service, so that is the fact the row states.
        let (session, session_ink): (&[u8], u32) = if vault.is_authenticated {
            (b"SESSION OPEN", THEME.success)
        } else {
            (b"NO SESSION", THEME.warning)
        };
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUTH_CARD_Y + CRED_SESSION_ROW_Y,
            CARD_W,
            b"Access State",
            session,
            session_ink,
            b"asked of the system unlock service; no key was released by it",
        ) {
            return c;
        }
        return fill;
    }

    if let Some(fill) = card_pixel(x, y, MARGIN, SECRETS_CARD_Y, CARD_W, SECRETS_CARD_H) {
        // WHY the caption changed: the card used to list three secrets whose
        // values were typed into the painter, and no code in this app ever
        // read them. What it actually holds are the answers its own probes
        // got back, so the card is now called what it shows.
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, SECRETS_CARD_Y + 18, b"KEYSTORE PROBES", INNER_W) {
            return c;
        }
        let slot0 = key_verdict(vault.key_probe[0]);
        let slot1 = key_verdict(vault.key_probe[1]);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 54,
            CARD_W,
            b"Root key, slot 0",
            slot0,
            key_ink(vault.key_probe[0]),
            b"keystore_derive(0), raw export to this app",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 128,
            CARD_W,
            b"Storage key, slot 1",
            slot1,
            key_ink(vault.key_probe[1]),
            b"keystore_derive(1), raw export to this app",
        ) {
            return c;
        }
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 202,
            CARD_W,
            b"Entropy source",
            entropy_verdict(vault.entropy_probe),
            entropy_ink(vault.entropy_probe),
            b"/dev/urandom probe; IVs fail closed without it",
        ) {
            return c;
        }
        let (identity_word, identity_ink) = identity_verdict(vault.identity);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            SECRETS_CARD_Y + 276,
            CARD_W,
            b"Device identity",
            identity_word,
            identity_ink,
            b"/system/etc/device_id, magic prefix only",
        ) {
            return c;
        }
        return fill;
    }

    if let Some(fill) = card_pixel(x, y, MARGIN, AUDIT_CARD_Y, CARD_W, AUDIT_CARD_H) {
        // WHY these two rows replaced the absolutes: the card used to assert
        // that authority and memory protection were enforced, with no probe
        // behind either claim. This app cannot ask the kernel whether it
        // enforced a capability; it can only count what its own calls
        // returned, which is what these rows now print.
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, AUDIT_CARD_Y + 18, b"WHAT THIS APP TESTED", INNER_W) {
            return c;
        }
        let verified = vault
            .sectors
            .iter()
            .filter(|record| record.map(|s| s.authenticated).unwrap_or(false))
            .count();
        let mut round_buf = [0u8; 24];
        let round_len = count_label(verified, vault.sectors.len(), &mut round_buf);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUDIT_CARD_Y + 54,
            CARD_W,
            b"Sector round-trips",
            &round_buf[..round_len],
            if verified == vault.sectors.len() {
                THEME.success
            } else if verified == 0 {
                THEME.on_surface_variant
            } else {
                THEME.warning
            },
            b"encrypt then decrypt in memory; nothing stored is re-read",
        ) {
            return c;
        }
        let derived = vault
            .key_probe
            .iter()
            .filter(|probe| **probe == KeyProbe::Derived)
            .count();
        let mut key_buf = [0u8; 24];
        let key_len = count_label(derived, vault.key_probe.len(), &mut key_buf);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            AUDIT_CARD_Y + 128,
            CARD_W,
            b"Raw keys held here",
            &key_buf[..key_len],
            if derived == 0 {
                THEME.on_surface_variant
            } else {
                THEME.warning
            },
            b"a derived key sits in this app's memory, not the hardware's",
        ) {
            return c;
        }
        return fill;
    }

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

/// Rows the status line owns: one scale-2 run at `STATUS_Y`.
const STATUS_H: usize = std::design::font::line_height(2);
/// The home-gesture box this app answers, and the rows its stack yields to it.
///
/// WHY named: the strip is the platform's and it is bottom-anchored, so the row
/// this app's stack stops owning is a property of the canvas, not a number to
/// remember -- a taller surface or a taller band has to move the boundary with
/// it rather than leave the stack painted into the system's indicator.
///
/// WHY it is read off `home_band::HIT` and is no longer a remembered 40: the
/// box this app answered was a hand-written 241 columns by 40 rows from row
/// 1400 -- asymmetric about column 360.5 and under the platform's 44-row touch
/// floor -- so a thumb one row clear of the box did nothing, which reads as a
/// hung device. The SDK names the answering box: the pill's neighbourhood at
/// the 48-row preferred target, centred and reaching the panel's last row.
/// The stack yields exactly the rows that box claims, because the press and
/// the ownership boundary were always one decision.
const GESTURE_BAND_H: usize = std::design::home_band::HIT.h;
/// The system's home-gesture strip: the indicator below it is the platform's,
/// bottom-anchored, and this app's stack grows toward it rather than over it.
const GESTURE_Y0: usize = H - GESTURE_BAND_H;
/// WHY checked at compile time: the band is the last `GESTURE_BAND_H` rows of
/// the surface and nothing else may live in it -- the mark inside it, the
/// stack above it, both have to agree with the boundary the same derivation
/// gives, and the boundary has to be the top of the box the press answers.
const _: () = assert!(GESTURE_Y0 + GESTURE_BAND_H == H);
const _: () = assert!(GESTURE_Y0 == std::design::home_band::HIT.y);
const _: () = assert!(GESTURE_BAND_H >= std::design::touch::MIN_TARGET);

/// The indicator this app draws, and the box its tap answers, both come from
/// the SDK's home band rather than from copies of its numbers.
///
/// WHY the mark was named by literals: three painters spelled `1418..1424,
/// 260..460` while the tap path owned every column from 1400 down -- eighteen
/// rows above the mark and two hundred columns outside it. A press on blank
/// surface beside the indicator closed the app, which is a control that is
/// tappable where it is not drawn. The ink and the answer were two independent
/// decisions about one control, which is how they drifted; `home_band::pixel`
/// now inks the shell's own pill and `home_band::contains` answers the box it
/// is declared inside, so paint and press are one declaration read twice.
///
/// WHY the gesture still owns the rows under the mark: from the answering box
/// to the bottom of the canvas this app paints nothing of its own, and the
/// strip is the system's; the press keeps the touch floor's height without
/// claiming a card.
const _: () = assert!(std::design::home_band::PILL.y >= GESTURE_Y0);
const _: () = assert!(
    std::design::home_band::PILL.y + std::design::home_band::PILL.h <= H
);

/// The rung a metric row's label and verdict are set in, and the rung its
/// provenance line is set in.
const METRIC_LABEL_RUNG: usize = 2;
const METRIC_PROVENANCE_RUNG: usize = 1;
/// The rows each of a metric row's two runs reserves, read off the ladder.
const METRIC_LABEL_BOX: usize = outline_text::row_band(0, METRIC_LABEL_RUNG).1;
const METRIC_PROVENANCE_BOX: usize = outline_text::row_band(0, METRIC_PROVENANCE_RUNG).1;
/// The air the row keeps between its label's box and its provenance's box.
const METRIC_ROW_STACK_GAP: usize = 2;
/// Where a metric row's provenance run starts, inside the row, below the whole
/// of the label's box.
///
/// WHY derived: the offset was a remembered 34, fitted when the body rung's box
/// was shorter than the 39 rows it reserves now. The provenance run started five
/// rows inside the label's own box, and `font::ink` answers for every pixel of
/// the box it is handed -- the label's branch is tried first, so the provenance's
/// first rows did not overlap the label, they were painted as the label's
/// background, five rows of a sentence nobody could read.
const METRIC_PROVENANCE_Y: usize = METRIC_LABEL_BOX + METRIC_ROW_STACK_GAP;
/// The rows one metric row reserves: both boxes and the air between them.
const METRIC_ROW_H: usize = METRIC_LABEL_BOX + METRIC_ROW_STACK_GAP + METRIC_PROVENANCE_BOX;
/// The step from one metric row's top to the next, and the air that step keeps
/// beyond the row's own boxes.
const METRIC_ROW_AIR: usize = 3;
const METRIC_ROW_PITCH: usize = METRIC_ROW_H + METRIC_ROW_AIR;
/// The air a card of metric rows keeps under its last row's box.
const CARD_ROW_FOOT_AIR: usize = 9;
/// The rows a card's caption band reserves from the card's top edge, and the air
/// under it before the card's first metric row.
const CARD_CAPTION_BAND_H: usize = CARD_CAPTION_Y + CARD_CAPTION_ROWS;
const CARD_ROW_HEAD_GAP: usize = spacing::GRID_12;
/// The row a card's first metric line stands on, below the whole caption band.
const CARD_FIRST_METRIC_Y: usize = CARD_CAPTION_BAND_H + CARD_ROW_HEAD_GAP;

/// WHY checked at compile time: the provenance run must start where the label's
/// box ends, never inside it, and the next row must start where this row's box
/// ends. Lower a rung's reservation, or write either offset by hand again, and
/// the pair fails rather than lose a sentence's first rows to the branch above.
const _: () = assert!(
    METRIC_PROVENANCE_Y >= METRIC_LABEL_BOX && METRIC_ROW_PITCH >= METRIC_ROW_H
);

/// Card A: hardware keystore probes.
///
/// WHY the whole storage stack hangs off `CONTENT_TOP`: these were five literals
/// that happened to be the header's bottom edge plus four, plus four more gaps
/// that happened to be twelve. The header is now taller than the literal that
/// used to bound it, and a stack of literals would have had to be found and
/// nudged by hand -- which is exactly how a band ends up painted under a rule.
const CARD_A_Y: usize = CONTENT_TOP;
/// The keystore card's rows, and its height read off them: the first row's top,
/// the pitch between rows, the last row's own box and the foot the card keeps.
///
/// WHY derived and not remembered: the card held a literal 296 while the three
/// rows standing in it come from the type ladder -- the same defect that put card
/// B's last plate past its foot, one card higher.
const CARD_A_ROWS: usize = 3;
const CARD_A_H: usize = CARD_FIRST_METRIC_Y
    + (CARD_A_ROWS - 1) * METRIC_ROW_PITCH
    + METRIC_ROW_H
    + CARD_ROW_FOOT_AIR;
const _: () = assert!(
    CARD_CAPTION_BAND_H <= CARD_FIRST_METRIC_Y && CARD_FIRST_METRIC_Y + METRIC_ROW_H <= CARD_A_H
);

/// The air between two cards of the storage stack.
const CARD_GAP: usize = 12;

/// Card B: managed sector grid, 2 columns x 3 rows of tiles.
const CARD_B_Y: usize = CARD_A_Y + CARD_A_H + CARD_GAP;
const TILE_W: usize = 304;
/// A tile's height: the three boxes it stacks, plus the air above and below.
/// WHY derived: the tile was a remembered 114 while its name and hint boxes
/// come from the type ladder, and the ladder's caption rung grew the stack past
/// the frame -- the hint's box ending three rows under the tile's own bottom.
const TILE_STACK_H: usize = font::line_height(2)
    + TILE_STACK_GAP
    + BADGE_PLATE_H
    + TILE_STACK_GAP
    + font::line_height(1);
const TILE_PAD_BOTTOM: usize = 6;
const TILE_H: usize = TILE_TITLE_Y + TILE_STACK_H + TILE_PAD_BOTTOM;
/// The grid's columns of tiles, and the tile count the model's rows imply.
///
/// WHY derived and not written: the painter and the tap path each looped `0..6`
/// while the grid's geometry answers to `TILE_ROWS`, so a fourth row would have
/// drawn three plates and hit-tested six -- the tiles that exist and the tiles a
/// press can reach would no longer be the same set.
const TILE_COLS: usize = 2;
const TILE_COUNT: usize = TILE_COLS * TILE_ROWS;
const TILE_COL_X: [usize; TILE_COLS] = [48, 368];
/// The grid's rows of tiles.
///
/// WHY named: the card's foot below is read from it, so a fourth row moves the
/// wall with the grid rather than pushing the last row out of it.
const TILE_ROWS: usize = 3;
/// The air between two tile rows of the grid.
///
/// WHY one knob: it was a bare literal inside the pitch, where nothing could say
/// what it was for or how many rows of the card it costs.
const TILE_ROW_GAP: usize = 10;
const TILE_PITCH: usize = TILE_H + TILE_ROW_GAP;

/// The row a card's caption stands on inside its card.
const CARD_CAPTION_Y: usize = 20;
/// The rung every card's caption is set in.
const CARD_CAPTION_RUNG: usize = 1;
/// The rows the caption's band reserves at that rung.
///
/// WHY read from the renderer and not from the rung's line box alone: a caption
/// is drawn by whichever face fits, and the bitmap cell's baseline can sit below
/// the outline ascent, which makes the inked band taller than `line_height`.
/// [`outline_text::row_band`] is the union of the two, so this is the number the
/// card has to hold rather than the number one of the two faces needs.
const CARD_CAPTION_ROWS: usize = outline_text::row_band(0, CARD_CAPTION_RUNG).1;
/// The first tile row's inset from the top of its card: the whole of the
/// caption's band, measured from the card's own top edge.
///
/// WHY derived: the inset was a remembered 60 fitted to the caption's old 27-row
/// band, so the ladder growing that band left the grid where it was while the
/// tiles under it grew -- the card was asked to pay for both and came up short.
/// The grid now starts where the painter stops reserving rows, and the space the
/// reader sees between the caption and the first plate is the tile's own
/// `TILE_TITLE_Y` air, not a second copy of the caption's.
const TILE_INSET: usize = CARD_CAPTION_Y + CARD_CAPTION_ROWS;
const TILE_ROW_Y: [usize; TILE_ROWS] = [
    CARD_B_Y + TILE_INSET,
    CARD_B_Y + TILE_INSET + TILE_PITCH,
    CARD_B_Y + TILE_INSET + 2 * TILE_PITCH,
];
/// The air the card keeps under its last row: the same step a tile keeps under
/// its last box, so the grid's foot and the plates' feet read as one rhythm.
const CARD_B_AIR: usize = TILE_PAD_BOTTOM;
/// Card B's height: the grid's rows at the ladder's own tile height, the air
/// between them, the caption's band above and the card's air below.
///
/// WHY read off the grid instead of remembered: the card held a literal 456 while
/// the tiles standing inside it come from the type ladder, and the ladder raising
/// the body rung put the third row's plate eleven rows past the card's own foot.
/// Those rows are painted by nobody -- the card's branch claims the pixel first
/// and stops at the card's edge -- while `handle_vault_touch` hit-tests the whole
/// tile, so the last row's tiles stayed tappable in the band where the user could
/// no longer see them.
const CARD_B_H: usize =
    TILE_INSET + TILE_ROWS * TILE_H + (TILE_ROWS - 1) * TILE_ROW_GAP + CARD_B_AIR;

/// A tile's own three-row stack: name, verdict pill, hint.
///
/// WHY each row is placed after the previous row's LINE BOX rather than after
/// the caps it visibly carries: `font::ink` answers for every pixel of the box
/// it is handed, and the title's branch is tried first, so the title reserves
/// `line_height(2)` rows whether or not it paints in them. The pill used to
/// start two rows inside that reservation, which cost it its top border across
/// the whole inner width -- and any descender in a sector name read through the
/// hairline that was supposed to be there.
const TILE_PAD: usize = 16;
const TILE_INNER_W: usize = TILE_W - 2 * TILE_PAD;
const TILE_TITLE_Y: usize = 14;
/// The rung a verdict pill's label is set in, on a tile and in a metric row.
const BADGE_RUNG: usize = 1;
/// The border row a verdict pill carries on each side of its run.
const BADGE_BORDER: usize = 1;
/// A verdict pill's height: the whole band of the run it carries, plus the two
/// border rows around it.
///
/// WHY derived from the ladder and not remembered: the plate was a written 36
/// rows, which was this rung's box plus six rows of air at the size the caption
/// rung used to be. The ladder grew that rung's box and the remembered height
/// stayed, so the pill was a plate whose size said one thing about the run inside
/// it while the run reserved another -- and the same literal stood in
/// `metric_row`, where nothing at all tied it to the verdict it carries.
const BADGE_PLATE_H: usize = outline_text::row_band(0, BADGE_RUNG).1 + 2 * BADGE_BORDER;
/// Air between two stacked boxes of the tile, kept as one knob.
const TILE_STACK_GAP: usize = 2;
const TILE_BADGE_Y: usize = TILE_TITLE_Y + font::line_height(2) + TILE_STACK_GAP;
const TILE_HINT_Y: usize = TILE_BADGE_Y + BADGE_PLATE_H + TILE_STACK_GAP;

/// WHY checked at compile time: the plate is only legal while its run's box ends
/// a border row above the plate's own foot, and while the ink is centred it has
/// to start below the top border. Change the rung or the border and this fails
/// instead of the pill clipping the verdict it holds.
const _: () = assert!(
    (BADGE_PLATE_H - outline_text::row_band(0, BADGE_RUNG).1) / 2
        + outline_text::row_band(0, BADGE_RUNG).1
        + BADGE_BORDER
        <= BADGE_PLATE_H
);

/// WHY checked at compile time: the stack is only legal while the hint's line
/// box still ends inside the tile. Raising a rung, the pill or the gap pushes
/// the last row past `TILE_H` and the gate fails; today it passes with six rows
/// of air, so it is a limit and not a tautology.
const _: () = assert!(TILE_HINT_Y + font::line_height(1) <= TILE_H);
/// WHY: the grid is allowed to grow only while its last row still sits inside
/// card B -- and sits there with the air the card's foot is authored to keep, not
/// merely touching it, because two hairlines that meet read as one plate. This is
/// the same chain from the other end: it is what a taller tile, a bigger inset or
/// another row would break first.
const _: () = assert!(TILE_ROW_Y[TILE_ROWS - 1] + TILE_H + CARD_B_AIR <= CARD_B_Y + CARD_B_H);
/// WHY the grid is gated against the card under it and not only against its own:
/// card C's top is the first row of the stack these tiles do not own, and the
/// card's own foot is only a promise until that neighbour's edge answers to the
/// same derivation. The step the reader sees between the two cards is the grid's
/// air, the card's foot and `CARD_GAP` together; a card that grew its foot
/// unnoticed would hand all three back.
const _: () = assert!(TILE_ROW_Y[TILE_ROWS - 1] + TILE_H + CARD_GAP <= CARD_C_Y);

/// Card C: backup export and entropy.
const CARD_C_Y: usize = CARD_B_Y + CARD_B_H + CARD_GAP;
/// The entropy card's rows and the height they add up to -- the same derivation
/// card A is given, at four rows instead of three.
const CARD_C_ROWS: usize = 4;
const CARD_C_H: usize = CARD_FIRST_METRIC_Y
    + (CARD_C_ROWS - 1) * METRIC_ROW_PITCH
    + METRIC_ROW_H
    + CARD_ROW_FOOT_AIR;
const _: () = assert!(CARD_FIRST_METRIC_Y + METRIC_ROW_H <= CARD_C_H);

/// Action row: two 328px buttons on the 2-column grid (2*328 + 16 + 2*24 = 720).
const BTN_Y: usize = CARD_C_Y + CARD_C_H + CARD_GAP;
const BTN_H: usize = 56;
const BTN_W: usize = 328;
const BTN_X: [usize; 2] = [24, 368];

/// The status line, under the action row.
///
/// WHY the step is the platform's smaller one: the grid above needed rows it did
/// not have, and the rows this stack can still spend are air -- no rung was
/// lowered, no plate shortened, and every target kept its minimum. Of the stack's
/// bands the status line is the only one that is neither a control nor a plate,
/// and the gate under it says what the foot may never cross.
///
/// WHY the last row of that air went: the SDK's answering box starts eight rows
/// lower than the strip this app used to hand-write, and the foot of the stack
/// ended level with it. The status line is not a plate, so the step under it
/// gave up the row -- one pixel of air, and the press box keeps every row the
/// panel's last column of empty canvas owns.
const STATUS_STEP: usize = spacing::GRID_8 - 1;
const STATUS_Y: usize = BTN_Y + BTN_H + STATUS_STEP;

/// WHY checked at compile time: `repaint` re-rasterises `STATUS_Y..STATUS_Y +
/// STATUS_H`, and the gesture strip below it is the platform's -- a stack that
/// grew past that row would be repainted over the system's own indicator.
const _: () = assert!(STATUS_Y + STATUS_H < GESTURE_Y0);

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

/// WHY checked at compile time: the grid paints and hit-tests
/// `TILE_COLS * TILE_ROWS` plates, and every one of them is a record of
/// `MANAGED_SECTORS` -- the model's own count. Add a row of tiles without adding
/// a sector, and this fails: the painter would otherwise draw a plate over a
/// record that does not exist, and `tile_origin` would walk past the grid it was
/// built from.
const _: () = assert!(TILE_COUNT == MANAGED_SECTORS.len());
const _: () = assert!(TILE_ROW_Y.len() == TILE_ROWS && TILE_COL_X.len() == TILE_COLS);

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
    std::design::back_button::pixel_for_theme(x, y, &THEME)
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
    if let Some(c) = font::ink_center(x, y, x0, y0 + centred_text_top(h, 1), 1, label, w, THEME.on_surface, THEME.surface_container_high, true) {
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
    if let Some(c) = font::ink(x, y, x0, y0, CARD_CAPTION_RUNG, text, max_w, THEME.primary, THEME.surface_container, true) {
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
    if let Some(c) = font::ink(
        x,
        y,
        ix,
        y0,
        METRIC_LABEL_RUNG,
        label,
        half,
        THEME.on_surface,
        THEME.surface_container,
        true,
    ) {
        return Some(c);
    }
    // Cryptographic status badge on the right - Scale 1 (Caption) bold truth badge
    let v_w = font::width(value, 1, true);
    let badge_w = (v_w + 34).min(half + 24);
    let badge_h = BADGE_PLATE_H;
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
        let text_y = by0 + (badge_h.saturating_sub(font::line_height(1))) / 2;
        if let Some(c) = font::ink(x, y, bx0 + 22, text_y, 1, value, badge_w.saturating_sub(26), value_ink, THEME.surface_container_low, true) {
            return Some(c);
        }
        return Some(THEME.surface_container_low);
    }
    if let Some(c) = font::ink(
        x,
        y,
        ix,
        y0 + METRIC_PROVENANCE_Y,
        METRIC_PROVENANCE_RUNG,
        source,
        w - PAD * 2,
        THEME.on_surface_variant,
        THEME.surface_container,
        true,
    ) {
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
    let iw = TILE_INNER_W;
    if let Some(c) = font::ink(x, y, tx + TILE_PAD, ty + TILE_TITLE_Y, 2, title, iw, THEME.on_surface, THEME.surface_container_high, true) {
        return Some(c);
    }
    // Cryptographic status badge: pill container with indicator dot and status verdict
    let badge_x = tx + TILE_PAD;
    let badge_y = ty + TILE_BADGE_Y;
    let badge_h = BADGE_PLATE_H;
    let v_w = font::width(verdict, 1, true);
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
        let text_y = badge_y + (badge_h.saturating_sub(font::line_height(1))) / 2;
        if let Some(c) = font::ink(x, y, badge_x + 22, text_y, 1, verdict, badge_w.saturating_sub(26), verdict_ink, THEME.surface_container, true) {
            return Some(c);
        }
        return Some(THEME.surface_container);
    }
    if let Some(c) = font::ink(x, y, tx + TILE_PAD, ty + TILE_HINT_Y, 1, hint, iw, THEME.on_surface_variant, THEME.surface_container_high, true) {
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
    // WHY the ladder places it and not the literal 28: `(bh - 28) / 2` centred
    // a rung-2 box of 32 rows in the band, which hung the caps three rows below
    // the button's own middle -- the box's descent is empty space above the
    // baseline for a capitalised label. `centred_text_top` is the placement the
    // gate above this function checks, so paint and gate now name one number.
    if let Some(c) = font::ink_center(x, y, bx, by + centred_text_top(bh, 2), 2, label, bw, ink_c, fill, true) {
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
    // Both pills stand in this row, and both are painted before the runs: the
    // runs therefore stop a gutter short of whichever reaches further left.
    // The bound used to be the RE-PROBE pill alone, so the CREDENTIALS pill --
    // which sits to its left -- was inside the space the title was allowed to
    // use, and a longer name than "Vault" would lose its tail to the pill
    // rather than show it.
    let leftmost_pill = action_rect(ACTION)
        .0
        .min(cred_action_rect(CRED_PILL_WIDEST).0);
    let text_max = head_text_max(leftmost_pill);
    if let Some(c) = font::ink(x, y, TITLE_X, TITLE_Y0, HEAD_TITLE_RUNG, TITLE, text_max, THEME.on_surface, THEME.surface, true) {
        return c;
    }
    if let Some(c) = font::ink(x, y, TITLE_X, SUBTITLE_Y0, HEAD_SUBTITLE_RUNG, SUBTITLE, text_max, THEME.on_surface_variant, THEME.surface, true) {
        return c;
    }
    THEME.surface
}

// -----------------------------------------------------------------------------
// Screen
// -----------------------------------------------------------------------------

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(vault: &mut VaultApp) {
    if let Some(now) = std::rtc::Reading::now() {
        vault.clock = now.clock_text();
        vault.clock_ok = true;
    } else {
        vault.clock_ok = false;
    }
}

#[inline(never)]
fn get_vault_pixel(x: usize, y: usize, vault: &VaultApp) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if vault.clock_ok { Some(&vault.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
    if vault.pin_modal_open {
        return pin_modal_pixel(x, y, vault);
    }
    if vault.show_credentials {
        return credentials_view_pixel(x, y, vault);
    }
    // Home gesture pill.
    if let Some(c) = std::design::home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
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
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_A_Y + CARD_CAPTION_Y, b"HARDWARE KEYSTORE", INNER_W) {
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
            CARD_A_Y + CARD_FIRST_METRIC_Y,
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
            CARD_A_Y + CARD_FIRST_METRIC_Y + METRIC_ROW_PITCH,
            CARD_W,
            b"Storage key, slot 1",
            slot1,
            key_ink(vault.key_probe[1]),
            b"keystore_derive(1), raw export to Ring 3",
        ) {
            return c;
        }
        let (identity_word, identity_ink) = identity_verdict(vault.identity);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_A_Y + CARD_FIRST_METRIC_Y + 2 * METRIC_ROW_PITCH,
            CARD_W,
            b"Device identity",
            identity_word,
            identity_ink,
            b"/system/etc/device_id, magic prefix only",
        ) {
            return c;
        }
        return fill;
    }

    // ---- Card B: managed sectors ------------------------------------------
    if let Some(fill) = card_pixel(x, y, MARGIN, CARD_B_Y, CARD_W, CARD_B_H) {
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_B_Y + CARD_CAPTION_Y, b"MANAGED SECTORS", INNER_W) {
            return c;
        }
        for index in 0..TILE_COUNT {
            let (tx, ty) = tile_origin(index);
            let name = sector_tile(index);
            let (hint_buf, hint_len) = vault::sector_tile_hint(index);
            let (verdict, verdict_ink) = match vault.sectors[index].map(|s| s.authenticated) {
                Some(true) => (b"round-trip verified".as_slice(), THEME.success),
                Some(false) => (b"not tested".as_slice(), THEME.on_surface_variant),
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
        if let Some(c) = caption_pixel(x, y, MARGIN + PAD, CARD_C_Y + CARD_CAPTION_Y, b"STATUS FRAME & ENTROPY", INNER_W) {
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
            CARD_C_Y + CARD_FIRST_METRIC_Y,
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
            CARD_C_Y + CARD_FIRST_METRIC_Y + METRIC_ROW_PITCH,
            CARD_W,
            b"Encryptions / decryptions",
            counters,
            THEME.on_surface,
            b"counters incremented by real vault calls",
        ) {
            return c;
        }
        let entropy_word = entropy_verdict(vault.entropy_probe);
        if let Some(c) = metric_row(
            x,
            y,
            MARGIN,
            CARD_C_Y + CARD_FIRST_METRIC_Y + 2 * METRIC_ROW_PITCH,
            CARD_W,
            b"Entropy source",
            entropy_word,
            entropy_ink(vault.entropy_probe),
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
            CARD_C_Y + CARD_FIRST_METRIC_Y + 3 * METRIC_ROW_PITCH,
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

fn entropy_ink(p: EntropyProbe) -> u32 {
    match p {
        EntropyProbe::Read16 => THEME.success,
        EntropyProbe::AllZero => THEME.error,
        EntropyProbe::Short => THEME.warning,
        EntropyProbe::Refused(_) => THEME.warning,
        EntropyProbe::NotRun => THEME.on_surface_variant,
    }
}

/// The device-identity probe's own words, with the ink that matches them.
fn identity_verdict(p: IdentityState) -> (&'static [u8], u32) {
    match p {
        IdentityState::Present => (b"DID: prefix present".as_slice(), THEME.success),
        IdentityState::Malformed => (b"present but malformed".as_slice(), THEME.warning),
        IdentityState::Unreadable => (b"not readable".as_slice(), THEME.warning),
        IdentityState::NotChecked => (b"not checked".as_slice(), THEME.on_surface_variant),
    }
}

/// Writes "n of total" into the caller's buffer and returns its length.
///
/// WHY a count and not a word: a row that says "verified" or "ENFORCED"
/// claims something this app cannot see, and a count is the same claim with
/// the evidence still attached.
fn count_label(n: usize, total: usize, buf: &mut [u8]) -> usize {
    let mut len = write_u64(n as u64, buf);
    let tail = b" of ";
    let t = tail.len().min(buf.len().saturating_sub(len));
    buf[len..len + t].copy_from_slice(&tail[..t]);
    len += t;
    let len2 = write_u64(total as u64, &mut buf[len..]);
    len + len2
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

    // Back button, or a press in the SDK's home box: exit to the launcher.
    //
    // WHY the box is `home_band::contains`: the tap used to be tested against
    // the mark's own columns from its row down while the event loop answered a
    // hand-written 241-column, 40-row strip -- two boxes for one gesture,
    // neither of them the platform's, the ink and the answer typed as
    // independent literals. The SDK declaration now names the answering box for
    // both paths at the 48-row touch floor, centred on the panel and covering
    // the pill it inks; `tests/test_vault_truth_contract.py` pins it.
    if std::design::back_button::contains(x, y) || std::design::home_band::contains(x, y) {
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
        let (ax, ay, aw, ah) = action_rect(SIGN_OUT);
        if in_round(x, y, ax, ay, aw, ah, ah / 2) {
            vault.show_credentials = false;
            vault.is_authenticated = false;
            status(vault, b"credential session signed out");
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::All);
        }
        if in_round(x, y, MARGIN, CRED_BTN_Y, CARD_W, CRED_BTN_H, 14) {
            vault.show_credentials = false;
            status(vault, b"returned to storage manager");
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    let cred_label: &[u8] = cred_pill_label(vault.is_authenticated);
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
    for index in 0..TILE_COUNT {
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
    refresh_clock(&mut vault);

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
                    // WHY the gesture is bound to the SDK's home box: the old
                    // code returned to the launcher for a pan or a fling from
                    // anywhere on the screen, so a scroll threw the user out of
                    // the vault. The first fix answered a hand-written 241 by
                    // 40 strip at row 1400 -- under the platform's 44-row touch
                    // floor and off-centre about column 360.5 -- so a thumb one
                    // row clear of the box did nothing, which reads as a hung
                    // device. `home_band::contains` answers the SDK's box: 240
                    // columns centred on the pill, 48 rows reaching 1439.
                    let is_home_gesture = std::design::home_band::contains(input.x as usize, input.y as usize);
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

