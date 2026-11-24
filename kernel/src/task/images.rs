// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Built-in userspace image registry.
//!
//! The ELF loader is generic; this module is the only kernel-facing boundary
//! that knows which first-party images are bundled into a particular system
//! image. Forks can replace this registry without changing process creation.

#[derive(Clone, Copy)]
pub struct UserImage {
    pub name: &'static str,
    pub elf: &'static [u8],
}

pub static USER_INIT_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/init"));
pub static APP_SETTINGS_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/settings"));
pub static DISPLAYD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/displayd"));
pub static INPUTD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/inputd"));
pub static STORAGED_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/storaged"));
pub static NETD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/netd"));
pub static POWERD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/powerd"));
pub static MODEMD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/modemd"));
pub static AUDIOD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/audiod"));
pub static CAMERAD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/camerad"));
pub static SENSORD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/sensord"));
pub static COMPOSITOR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/compositor"));
pub static LAUNCHER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/launcher"));
pub static APP_DIALER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/dialer"));
pub static APP_VAULT_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/vault"));
pub static APP_CAMERA_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/camera"));
pub static APP_CALCULATOR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/calculator"));
pub static APP_BROWSER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/browser"));
pub static APP_MESSAGES_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/messages"));
pub static APP_CALENDAR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/calendar"));
pub static APP_MUSIC_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/music"));
pub static APP_FILE_MANAGER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/file_manager"));
pub static APP_GALLERY_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/gallery"));
pub static APP_NOTES_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/notes"));
pub static APP_AGENT_MANAGER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/agent_manager"));
pub static APP_VIDEO_PLAYER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/video_player"));
pub static APP_WEBVIEW_DEMO_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/webview_demo"));
pub static AID_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/aid"));
pub static SEARCHD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/searchd"));
pub static AGENTD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/agentd"));
pub static ACRD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/acrd"));
pub static ACR_APP_RUNTIME_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/x86_64-unknown-none/debug/acr-app-runtime"));

// WHY the app statics above are embedded rather than loaded from the VFS: every
// first-party app already on the launch path (settings, browser, gallery, ...)
// is a kernel-resident image, and `sys_app_launch` spawns from that image rather
// than from a `/system/bin` file. Without these the launch arms would have no ELF
// to hand the Ring 3 loader, so the drawer entries could never start.

pub fn service_image(service_id: u8) -> Option<UserImage> {
    Some(match service_id {
        1 => UserImage { name: "displayd", elf: DISPLAYD_ELF },
        2 => UserImage { name: "inputd", elf: INPUTD_ELF },
        3 => UserImage { name: "storaged", elf: STORAGED_ELF },
        4 => UserImage { name: "netd", elf: NETD_ELF },
        5 => UserImage { name: "powerd", elf: POWERD_ELF },
        6 => UserImage { name: "modemd", elf: MODEMD_ELF },
        7 => UserImage { name: "audiod", elf: AUDIOD_ELF },
        8 => UserImage { name: "camerad", elf: CAMERAD_ELF },
        9 => UserImage { name: "compositor", elf: COMPOSITOR_ELF },
        10 => UserImage { name: "launcher", elf: LAUNCHER_ELF },
        11 => UserImage { name: "sensord", elf: SENSORD_ELF },
        12 => UserImage { name: "aid", elf: AID_ELF },
        13 => UserImage { name: "searchd", elf: SEARCHD_ELF },
        14 => UserImage { name: "agentd", elf: AGENTD_ELF },
        _ => return None,
    })
}

pub static ALL_USER_IMAGES: &[UserImage] = &[
    UserImage { name: "init", elf: USER_INIT_ELF },
    UserImage { name: "settings", elf: APP_SETTINGS_ELF },
    UserImage { name: "displayd", elf: DISPLAYD_ELF },
    UserImage { name: "inputd", elf: INPUTD_ELF },
    UserImage { name: "storaged", elf: STORAGED_ELF },
    UserImage { name: "netd", elf: NETD_ELF },
    UserImage { name: "powerd", elf: POWERD_ELF },
    UserImage { name: "modemd", elf: MODEMD_ELF },
    UserImage { name: "audiod", elf: AUDIOD_ELF },
    UserImage { name: "camerad", elf: CAMERAD_ELF },
    UserImage { name: "sensord", elf: SENSORD_ELF },
    UserImage { name: "compositor", elf: COMPOSITOR_ELF },
    UserImage { name: "launcher", elf: LAUNCHER_ELF },
    UserImage { name: "dialer", elf: APP_DIALER_ELF },
    UserImage { name: "vault", elf: APP_VAULT_ELF },
    UserImage { name: "camera", elf: APP_CAMERA_ELF },
    UserImage { name: "calculator", elf: APP_CALCULATOR_ELF },
    UserImage { name: "browser", elf: APP_BROWSER_ELF },
    UserImage { name: "messages", elf: APP_MESSAGES_ELF },
    UserImage { name: "calendar", elf: APP_CALENDAR_ELF },
    UserImage { name: "music", elf: APP_MUSIC_ELF },
    UserImage { name: "file_manager", elf: APP_FILE_MANAGER_ELF },
    UserImage { name: "gallery", elf: APP_GALLERY_ELF },
    UserImage { name: "notes", elf: APP_NOTES_ELF },
    UserImage { name: "agent_manager", elf: APP_AGENT_MANAGER_ELF },
    UserImage { name: "video_player", elf: APP_VIDEO_PLAYER_ELF },
    UserImage { name: "webview_demo", elf: APP_WEBVIEW_DEMO_ELF },
    UserImage { name: "aid", elf: AID_ELF },
    UserImage { name: "searchd", elf: SEARCHD_ELF },
    UserImage { name: "agentd", elf: AGENTD_ELF },
    UserImage { name: "acrd", elf: ACRD_ELF },
    UserImage { name: "acr-app-runtime", elf: ACR_APP_RUNTIME_ELF },
];

include!(concat!(env!("OUT_DIR"), "/embedded_image_digests.rs"));

/// Evaluates byte-by-byte equality of two 32-byte cryptographic digests in constant time.
#[inline(never)]
pub fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Validates ELF header invariants and verifies cryptographic SHA-256 digest
/// against the authenticated expected digest manifest.
///
/// Distinguishes three distinct security properties:
/// 1. Valid ELF structure (header magic, 64-bit ELFCLASS64, little-endian, ET_EXEC, x86_64 machine).
/// 2. Build-integrity consistency (cryptographic SHA-256 match against build-time expected digests).
/// 3. Authenticity based on an independently trusted signing authority (physical hardware secure boot).
pub fn verify_single_image(
    name: &str,
    elf: &[u8],
    expected_digest: Option<&[u8; 32]>,
) -> Result<(), &'static str> {
    use sha2::{Digest, Sha256};
    if elf.len() < 64 {
        return Err("image too small to be valid ELF (< 64 bytes)");
    }
    if &elf[0..4] != b"\x7fELF" {
        return Err("invalid ELF magic (expected \\x7fELF)");
    }
    if elf[4] != 2 {
        return Err("embedded user image is not 64-bit ELF (ELFCLASS64)");
    }
    if elf[5] != 1 {
        return Err("embedded user image is not little-endian (ELFDATA2LSB)");
    }
    if elf[6] != 1 {
        return Err("embedded user image has unsupported ELF version");
    }

    // ET_EXEC check (e_type == 2 at offset 16 in little endian)
    let e_type = u16::from_le_bytes([elf[16], elf[17]]);
    if e_type != 2 {
        return Err("embedded user image is not ET_EXEC");
    }

    // Machine check (e_machine == 0x3E for EM_X86_64 at offset 18)
    let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
    if e_machine != 0x3E {
        return Err("embedded user image has unsupported architecture (expected x86_64)");
    }

    // Locate expected digest: fail-closed if missing from manifest
    let expected = match expected_digest {
        Some(d) => d,
        None => match EXPECTED_IMAGE_DIGESTS.iter().find(|(n, _)| *n == name) {
            Some(&(_, ref d)) => d,
            None => return Err("missing digest manifest entry for embedded image"),
        },
    };

    // All-zero digest is invalid and fails closed
    if *expected == [0u8; 32] {
        return Err("all-zero expected digest is invalid (fail-closed)");
    }

    // Verify SHA-256 digest using constant-time comparison
    let actual_hash: [u8; 32] = Sha256::digest(elf).into();
    if !constant_time_eq_32(&actual_hash, expected) {
        return Err("embedded user image SHA-256 digest mismatch (corrupted or tampered binary)");
    }

    Ok(())
}

pub fn verify_embedded_images() -> Result<usize, &'static str> {
    // 1. Manifest / Image count invariant
    if ALL_USER_IMAGES.len() != EXPECTED_IMAGE_DIGESTS.len() {
        return Err("embedded image count mismatch with digest manifest");
    }

    // 2. Reject duplicate names in ALL_USER_IMAGES
    for i in 0..ALL_USER_IMAGES.len() {
        for j in (i + 1)..ALL_USER_IMAGES.len() {
            if ALL_USER_IMAGES[i].name == ALL_USER_IMAGES[j].name {
                return Err("duplicate image name in embedded manifest");
            }
        }
    }

    // 3. Reject duplicate names in EXPECTED_IMAGE_DIGESTS
    for i in 0..EXPECTED_IMAGE_DIGESTS.len() {
        for j in (i + 1)..EXPECTED_IMAGE_DIGESTS.len() {
            if EXPECTED_IMAGE_DIGESTS[i].0 == EXPECTED_IMAGE_DIGESTS[j].0 {
                return Err("duplicate entry in expected image digests");
            }
        }
    }

    // 4. Verify every single embedded image
    let mut verified_count = 0;
    for img in ALL_USER_IMAGES {
        verify_single_image(img.name, img.elf, None)?;
        verified_count += 1;
    }
    Ok(verified_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn make_test_elf(payload_byte: u8) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(b"\x7fELF");
        buf[4] = 2; // 64-bit (ELFCLASS64)
        buf[5] = 1; // Little-endian (ELFDATA2LSB)
        buf[6] = 1; // Current ELF version
        buf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC = 2
        buf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64 = 62
        buf[32] = payload_byte;
        buf
    }

    #[test]
    fn test_valid_image_verification_passes() {
        let elf = make_test_elf(0x42);
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert!(verify_single_image("test", &elf, Some(&digest)).is_ok());
    }

    #[test]
    fn test_constant_time_comparison() {
        let a = [0x5Au8; 32];
        let mut b = [0x5Au8; 32];
        assert!(constant_time_eq_32(&a, &b));
        b[31] ^= 1;
        assert!(!constant_time_eq_32(&a, &b));
    }

    #[test]
    fn test_tampered_payload_byte_causes_rejection() {
        let elf = make_test_elf(0x42);
        let digest: [u8; 32] = Sha256::digest(&elf).into();

        let mut tampered = elf;
        tampered[32] ^= 0xFF; // Modify single byte in payload

        let res = verify_single_image("test", &tampered, Some(&digest));
        assert_eq!(
            res,
            Err("embedded user image SHA-256 digest mismatch (corrupted or tampered binary)")
        );
    }

    #[test]
    fn test_all_zero_expected_digest_fails_closed() {
        let elf = make_test_elf(0x42);
        let zero_digest = [0u8; 32];
        let res = verify_single_image("test", &elf, Some(&zero_digest));
        assert_eq!(res, Err("all-zero expected digest is invalid (fail-closed)"));
    }

    #[test]
    fn test_missing_manifest_entry_fails_closed() {
        let elf = make_test_elf(0x42);
        let res = verify_single_image("non_existent_image_foo", &elf, None);
        assert_eq!(res, Err("missing digest manifest entry for embedded image"));
    }

    #[test]
    fn test_tampered_magic_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[0] = 0;
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("invalid ELF magic (expected \\x7fELF)")
        );
    }

    #[test]
    fn test_invalid_elf_class_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[4] = 1; // 32-bit ELFCLASS32
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("embedded user image is not 64-bit ELF (ELFCLASS64)")
        );
    }

    #[test]
    fn test_invalid_endianness_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[5] = 2; // Big-endian
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("embedded user image is not little-endian (ELFDATA2LSB)")
        );
    }

    #[test]
    fn test_invalid_elf_version_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[6] = 2; // Unsupported version
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("embedded user image has unsupported ELF version")
        );
    }

    #[test]
    fn test_tampered_executable_type_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN instead of ET_EXEC
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("embedded user image is not ET_EXEC")
        );
    }

    #[test]
    fn test_unsupported_machine_architecture_causes_rejection() {
        let mut elf = make_test_elf(0x42);
        elf[18..20].copy_from_slice(&183u16.to_le_bytes()); // EM_AARCH64 instead of EM_X86_64
        let digest: [u8; 32] = Sha256::digest(&elf).into();
        assert_eq!(
            verify_single_image("test", &elf, Some(&digest)),
            Err("embedded user image has unsupported architecture (expected x86_64)")
        );
    }

    #[test]
    fn test_truncated_image_causes_rejection() {
        let elf = [0x7fu8, b'E', b'L', b'F'];
        assert_eq!(
            verify_single_image("test", &elf, None),
            Err("image too small to be valid ELF (< 64 bytes)")
        );
    }
}
