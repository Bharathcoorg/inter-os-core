// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Built-in Userspace Image Registry
//!
//! Statically embeds first-party userspace binaries for AArch64.

pub struct UserImage {
    pub name: &'static str,
    pub elf: &'static [u8],
}

pub static USER_INIT_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/init"));
pub static DISPLAYD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/displayd"));
pub static INPUTD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/inputd"));
pub static STORAGED_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/storaged"));
pub static NETD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/netd"));
pub static POWERD_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/powerd"));
pub static COMPOSITOR_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/compositor"));
pub static LAUNCHER_ELF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/aarch64-unknown-none/debug/launcher"));

pub fn service_image(service_id: u8) -> Option<UserImage> {
    Some(match service_id {
        0 => UserImage { name: "init", elf: USER_INIT_ELF },
        1 => UserImage { name: "displayd", elf: DISPLAYD_ELF },
        2 => UserImage { name: "inputd", elf: INPUTD_ELF },
        3 => UserImage { name: "storaged", elf: STORAGED_ELF },
        4 => UserImage { name: "netd", elf: NETD_ELF },
        5 => UserImage { name: "powerd", elf: POWERD_ELF },
        9 => UserImage { name: "compositor", elf: COMPOSITOR_ELF },
        10 => UserImage { name: "launcher", elf: LAUNCHER_ELF },
        _ => return None,
    })
}
