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

// WHY the app statics above are embedded rather than loaded from the VFS: every
// first-party app already on the launch path (settings, terminal, gallery, ...)
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
        _ => return None,
    })
}
