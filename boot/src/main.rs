//! InterOS Core Disk Image Packaging Utility
//!
//! Takes the compiled `x86_64-unknown-none` kernel ELF binary and packages it into
//! a bootable BIOS/MBR disk image using the `bootloader` crate.

use anyhow::Context;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let kernel_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        let p1 = PathBuf::from("kernel/target/x86_64-unknown-none/debug/kernel");
        let p2 = PathBuf::from("target/x86_64-unknown-none/debug/kernel");
        if p1.exists() {
            p1
        } else {
            p2
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

    let bios_boot = bootloader::BiosBoot::new(&kernel_path);
    bios_boot
        .create_disk_image(&out_path)
        .context("Failed to create bootable BIOS disk image via bootloader crate")?;

    let file_size = std::fs::metadata(&out_path)?.len();
    println!("Status: SUCCESS! Bootable disk image created ({} bytes).", file_size);
    println!("============================================================\n");

    Ok(())
}
