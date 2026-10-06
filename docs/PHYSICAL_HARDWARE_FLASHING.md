# InterOS — Experimental Physical Hardware Bring-Up Guide

> [!CAUTION]
> **Experimental Research Bring-Up Only — Not Production Ready**  
> InterOS Core does **not** currently support production physical smartphone deployment. Verified runtime execution is established **only** on QEMU virtual targets (`qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`).  
> Attempting to flash unsupported consumer smartphones (such as retail Pixel, Qualcomm, or Tensor devices) carries a high risk of bricking your device or causing permanent data loss. Do **not** attempt to flash InterOS onto daily-driver hardware.

---

## 1. Architectural Reality & Verification State

In accordance with the [Platform Reality Contract](PLATFORM_REALITY_CONTRACT.md), device drivers and platform layers are explicitly classified:

| Subsystem | Driver / Architecture Layer | Implementation State | Verification State | Operational Reality |
| :--- | :--- | :--- | :--- | :--- |
| **CPU Privilege** | AArch64 EL1 (Supervisor) / EL0 (User) | Present | **QEMU-verified** | Configured in `entry.S` & `exceptions.rs` |
| **MMU / Paging** | Split tables (`TTBR0_EL1` / `TTBR1_EL1`) | Present | **QEMU-verified** | 39-bit VA space, `TCR_EL1`, `MAIR_EL1`, `SCTLR_EL1` |
| **Interrupts** | ARM GICv2 (Distributor & CPU Interface) | Present | **QEMU-verified** | Probed via Device Tree (FDT) |
| **Timer** | ARM Generic Timer (`CNTV_CTL_EL0`) | Present | **QEMU-verified** | Programmed via `CNTFRQ_EL0` (100 Hz scheduling) |
| **Firmware** | ARM PSCI v1.1 (`hvc #0` / `smc #0`) | Present | **QEMU-verified** | Core reset and shutdown in virtual machine |
| **Display Scanout** | SimpleFB Linear Framebuffer | Present | **QEMU-verified** | Software scanout via display HAL |
| **Multi-Touch** | Goodix GT911 Capacitive Digitizer | Experimental stub | **Source-only** | Dual-touch contact parsing logic; physical hardware unverified |
| **Cellular Modem** | 3GPP AT Serial Protocol Engine | Experimental stub | **Source-only** | Protocol parser state machine; physical baseband unverified |
| **Flash Storage** | SDHCI eMMC 5.1 Host Controller | Experimental stub | **Source-only** | Register definitions; physical eMMC unverified |
| **Power Management** | PMIC Fuel Gauge & DVFS Governor | Experimental stub | **Source-only** | Profile scaling logic; physical PMIC unverified |
| **Crypto Enclave** | Ed25519 & SHA-256 Attestation | Present | **Unit-tested** | Software cryptographic operations verified |

---

## 2. Experimental Target Artifacts

The build pipeline produces the following artifacts under `target/`:

```powershell
powershell -ExecutionPolicy Bypass -File qemu\build.ps1
```

- **`target/boot-aarch64.img`**: Standard Android Boot Image v2 header (`ANDROID!`), packaging the standalone AArch64 microkernel (`kernel-aarch64`), minimal initial ramdisk, and default Device Tree Blob. Produced for experimental laboratory boot testing.
- **`target/aarch64-unknown-none/debug/kernel-aarch64`**: Pure ELF64 binary suitable for direct JTAG or U-Boot network loading.
- **`target/inter-os-bios.img`**: Canonical bootable BIOS disk image for local x86_64 development and QEMU verification.

---

## 3. Experimental Laboratory Bring-Up (PinePhone Allwinner A64)

The primary experimental hardware development target is the **Pine64 PinePhone** (Allwinner A64 quad-core Cortex-A53), owing to its open bootloader (U-Boot), accessible hardware serial console, and non-destructive MicroSD boot capability.

### Non-Destructive MicroSD Boot (Laboratory Procedure)
To test kernel execution without writing to device eMMC or modifying internal partitions:

1. Insert a blank MicroSD card into your development workstation.
2. Partition the card with an MBR partition table:
   - **Partition 1**: FAT32, 64 MB (Bootable flag) — mounted as `/boot`.
   - **Partition 2**: ext4, remaining space.
3. Flash U-Boot SPL to the Allwinner boot offset:
   ```bash
   dd if=u-boot-sunxi-with-spl.bin of=/dev/sdX bs=1k seek=8
   ```
4. Copy `kernel-aarch64` and compiled Device Tree (`sun50i-a64-pinephone.dtb`) to Partition 1.
5. Create `/boot/boot.scr` with U-Boot boot instructions:
   ```text
   setenv bootargs console=ttyS0,115200 earlycon=uart8250,mmio32,0x01c28000
   fatload mmc 0:1 0x44000000 sun50i-a64-pinephone.dtb
   fatload mmc 0:1 0x40080000 kernel-aarch64
   booti 0x40080000 - 0x44000000
   ```
6. Insert the MicroSD card into the PinePhone and power on while monitoring serial output.

### Hardware Serial Debugging (Headphone Jack UART)
The PinePhone provides hardware UART access via the 3.5mm audio jack:
1. Ensure internal hardware DIP switch #6 (Headphone UART) is set to **ON**.
2. Connect a 3.3V USB-to-UART serial cable (Tip: TX, Ring 1: RX, Sleeve: GND).
3. Connect a serial terminal at `115200` baud, 8N1:
   ```bash
   picocom -b 115200 /dev/ttyUSB0
   ```

---

## 4. Boot Image Header Validation

The Fastboot flasher tool (`tools/flash_phone.ps1`) verifies image header consistency without modifying physical hardware:

```powershell
powershell -ExecutionPolicy Bypass -File tools\flash_phone.ps1 -Mode Info
```

*Illustrative example output (static format validation only — not physical device verification):*
```text
============================================================
  Inter OS -- Smartphone Hardware Flasher
============================================================
[IMAGE] Target Image: target\boot-aarch64.img
[IMAGE] Image Size:   10483712 bytes (10 MB)
[OK] Android Boot Image v2 format verified (Magic: 'ANDROID!').
[SUCCESS] Image header validation complete.
```

---

## 5. Summary of Experimental Limitations

- **No Retail Smartphone Support**: Retail Android smartphones with locked bootloaders, proprietary TrustZone enclaves, or unported proprietary display/touch controllers cannot run InterOS Core.
- **Cellular & Baseband**: 3GPP AT serial drivers are software-level protocol stubs; physical voice, SMS, or mobile data transmission on live networks is unverified.
- **Power Management**: Battery fuel gauges, thermals, and DVFS frequency governors are unverified on physical silicon.
- **Display Acceleration**: Only simple linear framebuffers are supported; hardware GPU drivers (Vulkan/DRM) are not implemented.

All developer workflows should use the canonical [QEMU Phone Platform](QEMU_PHONE_PLATFORM.md) for reproducible development.
