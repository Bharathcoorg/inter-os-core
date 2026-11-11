// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Built-in Userspace Image Registry
//!
//! Statically embeds first-party userspace binaries for RISC-V 64.

#[derive(Clone, Copy)]
pub struct UserImage {
    pub name: &'static str,
    pub elf: &'static [u8],
}

pub static USER_INIT_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/init"));
pub static DISPLAYD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/displayd"));
pub static INPUTD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/inputd"));
pub static STORAGED_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/storaged"));
pub static NETD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/netd"));
pub static POWERD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/powerd"));
pub static MODEMD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/modemd"));
pub static AUDIOD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/audiod"));
pub static CAMERAD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/camerad"));
pub static SENSORD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/sensord"));
pub static COMPOSITOR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/compositor"));
pub static LAUNCHER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/launcher"));
pub static AID_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/aid"));

pub static APP_SETTINGS_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/settings"));
pub static APP_CALCULATOR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/calculator"));
pub static APP_DIALER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/dialer"));
pub static APP_VAULT_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/vault"));
pub static APP_CAMERA_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/camera"));
pub static APP_BROWSER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/browser"));
pub static APP_MESSAGES_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/messages"));
pub static APP_CALENDAR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/calendar"));
pub static APP_MUSIC_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/music"));
pub static APP_FILE_MANAGER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/file_manager"));
pub static APP_GALLERY_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/gallery"));
pub static APP_NOTES_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/notes"));
pub static APP_AGENT_MANAGER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/agent_manager"));
pub static APP_VIDEO_PLAYER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/video_player"));
pub static APP_WEBVIEW_DEMO_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/riscv64gc-unknown-none-elf/debug/webview_demo"));

pub fn service_image(service_id: u8) -> Option<UserImage> {
    Some(match service_id {
        0 => UserImage { name: "init", elf: USER_INIT_ELF },
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
        _ => return None,
    })
}

pub fn app_image(name: &str) -> Option<UserImage> {
    Some(match name {
        "org.interos.settings" | "settings" | "app_settings" => UserImage { name: "app_settings", elf: APP_SETTINGS_ELF },
        "org.interos.calculator" | "calculator" | "app_calculator" => UserImage { name: "app_calculator", elf: APP_CALCULATOR_ELF },
        "org.interos.dialer" | "dialer" | "app_dialer" => UserImage { name: "app_dialer", elf: APP_DIALER_ELF },
        "org.interos.vault" | "vault" | "app_vault" => UserImage { name: "app_vault", elf: APP_VAULT_ELF },
        "org.interos.camera" | "camera" | "app_camera" => UserImage { name: "app_camera", elf: APP_CAMERA_ELF },
        "org.interos.browser" | "browser" | "app_browser" => UserImage { name: "app_browser", elf: APP_BROWSER_ELF },
        "org.interos.messages" | "messages" | "app_messages" => UserImage { name: "app_messages", elf: APP_MESSAGES_ELF },
        "org.interos.calendar" | "calendar" | "app_calendar" => UserImage { name: "app_calendar", elf: APP_CALENDAR_ELF },
        "org.interos.music" | "music" | "app_music" => UserImage { name: "app_music", elf: APP_MUSIC_ELF },
        "org.interos.file_manager" | "file_manager" | "files" | "app_file_manager" => UserImage { name: "app_file_manager", elf: APP_FILE_MANAGER_ELF },
        "org.interos.gallery" | "gallery" | "photos" | "app_gallery" => UserImage { name: "app_gallery", elf: APP_GALLERY_ELF },
        "org.interos.notes" | "notes" | "app_notes" => UserImage { name: "app_notes", elf: APP_NOTES_ELF },
        "org.interos.agentmanager" | "agentmanager" | "agent_manager" | "app_agent_manager" => UserImage { name: "app_agent_manager", elf: APP_AGENT_MANAGER_ELF },
        "org.interos.video_player" | "video_player" | "video" | "app_video_player" => UserImage { name: "app_video_player", elf: APP_VIDEO_PLAYER_ELF },
        "org.interos.webview_demo" | "webview_demo" | "webview" | "app_webview_demo" => UserImage { name: "app_webview_demo", elf: APP_WEBVIEW_DEMO_ELF },
        _ => return None,
    })
}
