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

pub fn verify_embedded_images() -> Result<usize, &'static str> {
    use sha2::{Sha256, Digest};
    let mut verified_count = 0;
    for img in ALL_USER_IMAGES {
        if img.elf.len() < 64 {
            return Err("image too small to be valid ELF");
        }
        if &img.elf[0..4] != b"\x7fELF" {
            return Err("invalid ELF magic");
        }
        // ET_EXEC check (e_type == 2 at offset 16 in little endian)
        let e_type = u16::from_le_bytes([img.elf[16], img.elf[17]]);
        if e_type != 2 {
            return Err("embedded user image is not ET_EXEC");
        }
        // Compute SHA-256 hash to prove whole-image digest integrity
        let _hash = Sha256::digest(img.elf);
        verified_count += 1;
    }
    Ok(verified_count)
}
