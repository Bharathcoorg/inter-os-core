// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Core Disk Image Packaging Utility
//!
//! Takes the compiled `x86_64-unknown-none` kernel ELF binary and packages it into
//! a bootable BIOS/MBR disk image using the `bootloader` crate.

use anyhow::Context;
use bootloader::{BiosBoot, BootConfig};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let kernel_path = {
        let arg_path = if args.len() > 1 {
            Some(PathBuf::from(&args[1]))
        } else {
            None
        };
        if let Some(ap) = arg_path {
            if ap.exists() {
                ap
            } else {
                anyhow::bail!("Specified kernel ELF path does not exist: {:?}", ap);
            }
        } else {
            let p1 = PathBuf::from("kernel/target/x86_64-unknown-none/release/kernel");
            let p2 = PathBuf::from("target/x86_64-unknown-none/release/kernel");
            let p3 = PathBuf::from("kernel/target/x86_64-unknown-none/debug/kernel");
            let p4 = PathBuf::from("target/x86_64-unknown-none/debug/kernel");
            if p1.exists() {
                p1
            } else if p2.exists() {
                p2
            } else if p3.exists() {
                p3
            } else if p4.exists() {
                p4
            } else {
                anyhow::bail!("Kernel ELF not found in standard target locations");
            }
        }
    };

    let out_path = if args.len() > 2 {
        PathBuf::from(&args[2])
    } else {
        PathBuf::from("target/inter-os-bios.img")
    };

    println!("============================================================");
    println!("  [InterOS Core Disk Builder] Creating Bootable Disk Image");
    println!("============================================================");
    println!("Kernel Binary: {}", kernel_path.display());
    println!("Target Image:  {}", out_path.display());

    anyhow::ensure!(
        kernel_path.exists(),
        "Kernel ELF binary does not exist at {:?}. Please run cargo build --manifest-path kernel/Cargo.toml first.",
        kernel_path
    );

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .context("Failed to create target output directory")?;
    }

    // Prefer the launcher logical surface at the bootloader boundary. This is a
    // minimum request, not a fixed mode: unsupported firmware may choose a
    // smaller framebuffer, which the runtime reports instead of inventing one.
    let mut boot_config = BootConfig::default();
    boot_config.frame_buffer.minimum_framebuffer_width = Some(720);
    boot_config.frame_buffer.minimum_framebuffer_height = Some(1440);
    boot_config.frame_buffer_logging = false;

    let mut bios_boot = BiosBoot::new(&kernel_path);
    bios_boot
        .set_boot_config(&boot_config)
        .create_disk_image(&out_path)
        .context("Failed to create bootable BIOS disk image via bootloader crate")?;

    let file_size = std::fs::metadata(&out_path)?.len();
    println!("Status: SUCCESS! Bootable disk image created ({} bytes).", file_size);
    println!("============================================================\n");

    Ok(())
}
