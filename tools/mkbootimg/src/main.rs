// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Android Boot Image Packaging Utility (`mkbootimg`)
//!
//! Packages compiled microkernel ELF/binaries, initramfs ramdisk, and DTB
//! into an official Android Boot Image (Header v2) that can be flashed
//! directly to physical smartphones via `fastboot flash boot boot.img`.
//!
//! Follows Android Open Source Project (AOSP) specification with zero dependencies.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const BOOT_MAGIC: &[u8; 8] = b"ANDROID!";
const DEFAULT_PAGE_SIZE: usize = 2048;

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct BootImgHeaderV2 {
    magic: [u8; 8],
    kernel_size: u32,
    kernel_addr: u32,
    ramdisk_size: u32,
    ramdisk_addr: u32,
    second_size: u32,
    second_addr: u32,
    tags_addr: u32,
    page_size: u32,
    header_version: u32,
    os_version: u32,
    name: [u8; 16],
    cmdline: [u8; 512],
    id: [u8; 32],
    extra_cmdline: [u8; 1024],
    recovery_dtbo_size: u32,
    recovery_dtbo_offset: u64,
    header_size: u32,
    dtb_size: u32,
    dtb_addr: u64,
}

// Pure Rust SHA-256 implementation
struct Sha256 {
    state: [u32; 8],
    count: u64,
    buffer: [u8; 64],
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            count: 0,
            buffer: [0u8; 64],
        }
    }

    fn update(&mut self, data: &[u8]) {
        let mut i = 0;
        let mut idx = (self.count & 63) as usize;
        self.count += data.len() as u64;

        if idx > 0 && idx + data.len() >= 64 {
            let take = 64 - idx;
            self.buffer[idx..64].copy_from_slice(&data[..take]);
            self.process_block();
            i += take;
            idx = 0;
        }

        while i + 64 <= data.len() {
            self.buffer.copy_from_slice(&data[i..i + 64]);
            self.process_block();
            i += 64;
        }

        if i < data.len() {
            let rem = data.len() - i;
            self.buffer[idx..idx + rem].copy_from_slice(&data[i..]);
        }
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.count * 8;
        let idx = (self.count & 63) as usize;
        self.buffer[idx] = 0x80;
        if idx + 1 > 56 {
            for b in &mut self.buffer[idx + 1..64] {
                *b = 0;
            }
            self.process_block();
            for b in &mut self.buffer[..56] {
                *b = 0;
            }
        } else {
            for b in &mut self.buffer[idx + 1..56] {
                *b = 0;
            }
        }
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        self.process_block();

        let mut out = [0u8; 32];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&s.to_be_bytes());
        }
        out
    }

    fn process_block(&mut self) {
        let k: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
            0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
            0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
            0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
            0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
            0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
            0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        ];

        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                self.buffer[i * 4],
                self.buffer[i * 4 + 1],
                self.buffer[i * 4 + 2],
                self.buffer[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        let mut f = self.state[5];
        let mut g = self.state[6];
        let mut h = self.state[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

fn pad_to_page(vec: &mut Vec<u8>, page_size: usize) {
    let remainder = vec.len() % page_size;
    if remainder != 0 {
        let pad_len = page_size - remainder;
        vec.resize(vec.len() + pad_len, 0);
    }
}

fn read_file_or_empty(path: &Path) -> Vec<u8> {
    if path.exists() {
        let mut buf = Vec::new();
        if let Ok(mut f) = File::open(path) {
            let _ = f.read_to_end(&mut buf);
        }
        buf
    } else {
        Vec::new()
    }
}

/// ELF `e_machine` values this tool can reason about.
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const EM_RISCV: u16 = 243;

fn arch_name(machine: u16) -> &'static str {
    match machine {
        EM_X86_64 => "x86_64",
        EM_AARCH64 => "AArch64 (ARM64)",
        EM_RISCV => "RISC-V",
        3 => "x86",
        40 => "ARM",
        _ => "unknown",
    }
}

/// Reads the ELF `e_machine` field so a phone image can never silently contain a
/// kernel for the wrong processor.
///
/// WHY this exists: an Android boot image targets a physical smartphone, which is
/// always ARM64. This tool previously packaged whatever file it was handed, so
/// `qemu/build.ps1` produced `boot.img` from the *x86_64* kernel and then
/// printed Pixel/PinePhone flash instructions for it. The container was
/// well-formed and would have flashed without complaint, and the device would
/// have failed to boot with no explanation. Reading `e_machine` turns that
/// silent mismatch into a refusal.
fn elf_machine(data: &[u8]) -> Option<u16> {
    // e_ident is 16 bytes; e_machine sits at offset 18 and is 2 bytes.
    if data.len() < 20 || &data[0..4] != b"\x7fELF" {
        return None;
    }
    let bytes = [data[18], data[19]];
    match data[5] {
        1 => Some(u16::from_le_bytes(bytes)), // ELFDATA2LSB
        2 => Some(u16::from_be_bytes(bytes)), // ELFDATA2MSB
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut kernel_path = PathBuf::from("target/x86_64-unknown-none/debug/kernel");
    let mut ramdisk_path = PathBuf::from("target/ramdisk.cpio");
    let mut dtb_path = PathBuf::from("target/device_tree.dtb");
    let mut out_path = PathBuf::from("target/boot.img");
    let mut cmdline = "console=ttyAMA0,115200 root=/dev/ram0 capability=strict zero_trust=enforce".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--kernel" => {
                if i + 1 < args.len() { kernel_path = PathBuf::from(&args[i + 1]); i += 1; }
            }
            "--ramdisk" => {
                if i + 1 < args.len() { ramdisk_path = PathBuf::from(&args[i + 1]); i += 1; }
            }
            "--dtb" => {
                if i + 1 < args.len() { dtb_path = PathBuf::from(&args[i + 1]); i += 1; }
            }
            "--cmdline" => {
                if i + 1 < args.len() { cmdline = args[i + 1].clone(); i += 1; }
            }
            "--output" => {
                if i + 1 < args.len() { out_path = PathBuf::from(&args[i + 1]); i += 1; }
            }
            _ => {}
        }
        i += 1;
    }

    println!("============================================================");
    println!("  [Inter OS] Android Boot Image Generator (Fastboot v2)");
    println!("============================================================");
    println!("Container target: AArch64 physical smartphone (Pixel, PinePhone, Snapdragon)");
    println!("Kernel Binary: {}", kernel_path.display());
    println!("Output Target: {}", out_path.display());

    let kernel = read_file_or_empty(&kernel_path);
    if kernel.is_empty() {
        eprintln!("[ERROR] Kernel image not found at {:?}.", kernel_path);
        std::process::exit(1);
    }

    // WHY this gate: an Android boot image is only ever loaded by an ARM64
    // phone. Refusing a mismatched kernel here is the difference between a
    // clear error and a device that flashes fine and then does nothing.
    let machine = match elf_machine(&kernel) {
        Some(m) => m,
        None => {
            eprintln!("[ERROR] {} is not an ELF executable, so its architecture cannot be verified.", kernel_path.display());
            eprintln!("        Refusing to package it into a device boot image.");
            std::process::exit(1);
        }
    };
    if machine != EM_AARCH64 {
        eprintln!("[ERROR] Refusing to package a non-AArch64 kernel into a smartphone boot image.");
        eprintln!("        {}: ELF e_machine = {} ({})", kernel_path.display(), machine, arch_name(machine));
        eprintln!("        Physical smartphones are ARM64. This image would flash and then not boot.");
        std::process::exit(1);
    }
    println!("Kernel architecture: {} (verified from ELF e_machine)", arch_name(machine));

    // WHY these two refuse instead of synthesising: the tool used to substitute
    // the FDT magic `d00dfeed` followed by the ASCII string
    // "SOVEREIGN_DEVICE_TREE_BLOB_MOBILE_DEVBOARD_V1", and the CPIO magic
    // "070701" followed by "INTER_OS_SOVEREIGN_RAMDISK_CONTAINING_INIT_SETTINGS_TERMINAL_DIALER_RETAIL_APK".
    // Neither was a real device tree or a real cpio archive, yet the header
    // reported non-zero `dtb_size`/`ramdisk_size`, so the image claimed to carry
    // a device tree and an init/terminal/dialer ramdisk that did not exist.
    // Missing payloads are now a hard error naming exactly what is absent.
    let mut missing: Vec<String> = Vec::new();

    let ramdisk = read_file_or_empty(&ramdisk_path);
    if ramdisk.is_empty() {
        missing.push(format!("ramdisk ({})", ramdisk_path.display()));
    }

    let dtb = read_file_or_empty(&dtb_path);
    if dtb.is_empty() {
        missing.push(format!("device tree blob ({})", dtb_path.display()));
    }

    if !missing.is_empty() {
        eprintln!("[ERROR] Cannot build a smartphone boot image: required payloads are missing.");
        for m in &missing {
            eprintln!("        missing: {}", m);
        }
        eprintln!("        Placeholders are no longer synthesised, because a boot image that reports a");
        eprintln!("        non-zero dtb_size/ramdisk_size while carrying neither is a silent lie that");
        eprintln!("        reaches the device as a boot failure with no usable diagnosis.");
        std::process::exit(1);
    }

    let page_size = DEFAULT_PAGE_SIZE;
    let mut img = Vec::new();

    let mut hdr = BootImgHeaderV2 {
        magic: *BOOT_MAGIC,
        kernel_size: kernel.len() as u32,
        kernel_addr: 0x80008000,
        ramdisk_size: ramdisk.len() as u32,
        ramdisk_addr: 0x82000000,
        second_size: 0,
        second_addr: 0,
        tags_addr: 0x80000100,
        page_size: page_size as u32,
        header_version: 2,
        os_version: (14 << 25) | (9 << 14) | 2026,
        name: [0u8; 16],
        cmdline: [0u8; 512],
        id: [0u8; 32],
        extra_cmdline: [0u8; 1024],
        recovery_dtbo_size: 0,
        recovery_dtbo_offset: 0,
        header_size: std::mem::size_of::<BootImgHeaderV2>() as u32,
        dtb_size: dtb.len() as u32,
        dtb_addr: 0x80000000,
    };

    let name_bytes = b"INTER_OS_AARCH64";
    hdr.name[..name_bytes.len()].copy_from_slice(name_bytes);

    let cmd_bytes = cmdline.as_bytes();
    let len = cmd_bytes.len().min(511);
    hdr.cmdline[..len].copy_from_slice(&cmd_bytes[..len]);

    // Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&kernel);
    let len_k = (kernel.len() as u32).to_le_bytes();
    hasher.update(&len_k);

    hasher.update(&ramdisk);
    let len_r = (ramdisk.len() as u32).to_le_bytes();
    hasher.update(&len_r);

    if !dtb.is_empty() {
        hasher.update(&dtb);
        let len_d = (dtb.len() as u32).to_le_bytes();
        hasher.update(&len_d);
    }
    let digest = hasher.finalize();
    hdr.id.copy_from_slice(&digest);

    // 1. Header Page
    let hdr_bytes = unsafe {
        std::slice::from_raw_parts(
            &hdr as *const BootImgHeaderV2 as *const u8,
            std::mem::size_of::<BootImgHeaderV2>(),
        )
    };
    img.extend_from_slice(hdr_bytes);
    pad_to_page(&mut img, page_size);

    // 2. Kernel
    img.extend_from_slice(&kernel);
    pad_to_page(&mut img, page_size);

    // 3. Ramdisk
    img.extend_from_slice(&ramdisk);
    pad_to_page(&mut img, page_size);

    // 4. DTB
    img.extend_from_slice(&dtb);
    pad_to_page(&mut img, page_size);

    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut out_file = File::create(&out_path).expect("Failed to create output boot.img");
    out_file.write_all(&img).expect("Failed to write boot.img");

    println!("Status: SUCCESS! Android boot image created: {} ({} bytes)", out_path.display(), img.len());
    println!("SHA-256 Partition ID: {:02x?}", &digest[..8]);
    println!("\nFlashing this image (UNTESTED on hardware -- no Inter OS build has booted on a physical device):");
    println!("  1. Unlock the bootloader (device-specific; erases user data).");
    println!("  2. Connect the phone in fastboot mode and confirm: fastboot devices");
    println!("  3. fastboot flash boot {}", out_path.display());
    println!("  4. fastboot reboot");
    println!("\nA well-formed container is not a working OS: this kernel has no display, touch, or");
    println!("storage driver for real phone hardware, so a successful flash is not expected to");
    println!("reach a usable UI.");
    println!("============================================================\n");
}
