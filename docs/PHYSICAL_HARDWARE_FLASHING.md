# InterOS — Platform Image & Bring-Up Status

> [!CAUTION]
> **Research Prototype Only — Execution in QEMU Only**  
> InterOS Core is an open-source research prototype of a smartphone operating system, not a commercial product. Verified runtime execution is established **only** in QEMU virtual targets (`qemu-system-x86_64`, `qemu-system-aarch64`, `qemu-system-riscv64`).  
> There is no physical device, no silicon execution, and no hardware telemetry. InterOS Core cannot be flashed to consumer smartphones or retail devices, and physical device flashing is not supported. The build produces a QEMU disk image and nothing else. External contributions are not accepted at this stage and will open after the first stable release.

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
| **Key Sealing** | Ed25519 & SHA-256 Attestation (software) | Present | **Unit-tested** | `kernel/src/crypto/keystore.rs` reports `is_hardware_backed() == false`; software derivation only |

---

## 2. Target Build Artifacts

Each architecture is built and verified as a QEMU artifact:

```powershell
python tools/interos.py build demo       # x86_64 canonical QEMU disk
python tools/interos.py build aarch64    # AArch64 flat Image for qemu-system-aarch64
python tools/interos.py build riscv64    # RISC-V 64 ELF for qemu-system-riscv64
```

- **`target/inter-os-bios.img`**: Canonical bootable BIOS disk image for local x86_64 development and QEMU verification. This is the artifact every runtime suite boots.
- **`target/kernel-aarch64.img`**: Flat arm64 `Image` packed from the `kernel-aarch64` ELF; loaded by `qemu-system-aarch64 -machine virt` at `loader_start + text_offset`. A QEMU boot artifact, not a flashable phone partition.
- **`target/riscv64gc-unknown-none-elf/debug/kernel-riscv64`**: ELF64 binary loaded by `qemu-system-riscv64 -machine virt` with OpenSBI.
- **No phone boot image**: `target/boot-aarch64.img` is not produced by this tree, and nothing packages `kernel-aarch64` into an Android Boot Image. An ARM64 device image additionally needs a genuine device tree blob and a genuine initramfs; neither exists here, so `tools/mkbootimg` refuses to run rather than synthesise placeholders, and `qemu/build.ps1` reports `[BLOCKED] No phone boot image was produced`.

---

## 3. Architecture Bring-Up Targets (Unverified on Physical Silicon)

Source-level architectural targets exist for AArch64 and RISC-V 64:

- **AArch64 (`kernel-aarch64`)**: Boot-verified inside QEMU `virt` (Cortex-A53). Board-specific peripheral initialization for physical hardware (such as Allwinner A64 / PinePhone) is unverified source code; no image has been flashed to physical silicon or tested on a physical handset. The AArch64 kernel has no display, touch, or phone storage driver, so a successful flash would not be expected to reach a usable interface.
- **RISC-V 64 (`kernel-riscv64`)**: Boot-verified inside QEMU `virt` (RV64). Physical SBC bring-up is unverified.
- **No Verified Physical Flashing Workflow**: The flashing procedures this document used to describe — partitioning an SD card, writing U-Boot with `dd`, wiring a UART adapter to the PinePhone headphone jack — were theoretical steps that were never executed against hardware. The repository's flasher scripts are described in section 4, and the image they would need does not build.

### Serial Diagnostics
Serial UART output is observed exclusively through the QEMU emulator console (TCP port 4301 or stdio). There are no physical UART connections or hardware adapters supported.

---

## 4. Boot Image Header Validation

`tools/flash_phone.ps1` and `tools/flash_phone.sh` are real Fastboot drivers, not inspectors: they invoke `fastboot devices`, `fastboot getvar`, and — under `-Mode Boot` / `-Mode Flash` (the latter gated behind `-AllowPersistentFlash`) — `fastboot boot` and `fastboot flash`. What the contract suite pins is their validation behaviour, because the check alone is not a readiness claim:

- Both refuse a container whose payloads are not structurally real, including the magic-plus-ASCII device tree and ramdisk this repository used to ship.
- Both accept a structurally valid container, so the check discriminates rather than always refusing.
- Both print `[NOT READY] This platform has never been booted on physical hardware.` on success, and the packer refuses a non-AArch64 kernel so an x86_64 build can never be packaged for ARM64 hardware again.

```powershell
powershell -ExecutionPolicy Bypass -File tools\flash_phone.ps1 -Mode Info
```

Against the current tree that command stops at its own image check: `target\boot-aarch64.img` is absent, `qemu/build.ps1` cannot produce it, and the script exits with `Target boot image ... was not found and could not be built`. That refusal is the accurate state of an unfinished platform, and no example output is quoted here because a successful run cannot be reproduced from this repository.

---

## 5. Summary of Experimental Limitations

- **No Retail Smartphone Support**: Retail Android smartphones with locked bootloaders, proprietary TrustZone enclaves, or unported proprietary display/touch controllers cannot run InterOS Core.
- **Cellular & Baseband**: 3GPP AT serial drivers are software-level protocol stubs; physical voice, SMS, or mobile data transmission on live networks is unverified.
- **Power Management**: Battery fuel gauges, thermals, and DVFS frequency governors are unverified on physical silicon.
- **Display Acceleration**: Only simple linear framebuffers are supported; hardware GPU drivers (Vulkan/DRM) are not implemented.

All developer workflows should use the canonical [QEMU Phone Platform](QEMU_PHONE_PLATFORM.md) for reproducible development.
