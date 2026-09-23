#!/usr/bin/env bash
# ==============================================================================
# Inter OS — Sovereign Smartphone Hardware Flashing Tool (AArch64 Fastboot Automator)
# ==============================================================================
# Flashes or tether-boots Inter OS onto physical smartphone hardware (PinePhone,
# Qualcomm Snapdragon, Google Pixel, MediaTek Dimensity) using the standard
# Android Fastboot protocol (AOSP Boot Image v2).
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_IMG="${1:-"$SCRIPT_DIR/../target/boot-aarch64.img"}"
MODE="${2:-"boot"}"

echo "============================================================"
echo "  Inter OS — Sovereign Smartphone Hardware Flasher v4.6"
echo "============================================================"

# 1. Locate Fastboot binary
if ! command -v fastboot &> /dev/null; then
    echo "[WARNING] 'fastboot' was not found in PATH."
    echo "Please install android-tools or Android Platform Tools."
    echo "Flashing commands for manual execution:"
    echo "  fastboot flash boot \"$TARGET_IMG\""
    echo "  fastboot reboot"
    exit 1
fi

FASTBOOT="$(command -v fastboot)"
echo "[OK] Fastboot found at: $FASTBOOT"

# 2. Check Image File
if [ ! -f "$TARGET_IMG" ]; then
    echo "[BUILDING] Image not found at '$TARGET_IMG'. Compiling now..."
    "$SCRIPT_DIR/../qemu/build.ps1" || cargo +nightly run --manifest-path "$SCRIPT_DIR/mkbootimg/Cargo.toml" -- --kernel "$SCRIPT_DIR/../target/aarch64-unknown-none/debug/kernel-aarch64" --output "$TARGET_IMG"
fi

FILE_SIZE=$(wc -c < "$TARGET_IMG" | tr -d ' ')
SHA256_HASH=$(sha256sum "$TARGET_IMG" | awk '{print $1}')

echo "[IMAGE] Target Image: $TARGET_IMG"
echo "[IMAGE] Image Size:   $FILE_SIZE bytes"
echo "[IMAGE] SHA-256 Hash: $SHA256_HASH"

# Verify header magic
MAGIC=$(head -c 8 "$TARGET_IMG")
if [ "$MAGIC" != "ANDROID!" ]; then
    echo "[ERROR] Invalid boot image! Expected 'ANDROID!', got '$MAGIC'."
    exit 1
fi
echo "[OK] Android Boot Image v2 format verified (Magic: 'ANDROID!')."

# 3. Check Connected Physical Devices
echo ""
echo "--- [Querying Connected Physical Smartphone Devices] ---"
DEVICES=$("$FASTBOOT" devices || true)

if [ -z "$DEVICES" ]; then
    echo "[WAITING] No devices found in Fastboot mode."
    echo "Please connect your phone via USB and enter Fastboot mode (adb reboot bootloader)."
    echo ""
    echo "Direct Flash Commands (run once connected):"
    if [ "$MODE" = "boot" ]; then
        echo "  fastboot boot \"$TARGET_IMG\""
    else
        echo "  fastboot flash boot \"$TARGET_IMG\""
        echo "  fastboot reboot"
    fi
    exit 0
fi

echo "[DEVICE] Connected Fastboot Devices:"
echo "$DEVICES"

# 4. Perform Operation
echo ""
if [ "$MODE" = "boot" ]; then
    echo "[OPERATION] Initiating Tethered Live RAM Boot (fastboot boot)..."
    "$FASTBOOT" boot "$TARGET_IMG"
    echo "[SUCCESS] Inter OS kernel loaded and booted on physical smartphone!"
elif [ "$MODE" = "flash" ]; then
    echo "[OPERATION] Flashing Inter OS to physical boot partition..."
    "$FASTBOOT" flash boot "$TARGET_IMG"
    echo "[SUCCESS] Boot partition flashed successfully!"
    echo "Rebooting smartphone into Inter OS..."
    "$FASTBOOT" reboot
    echo "[DONE] Smartphone rebooted. Welcome to Sovereign Inter OS."
fi
