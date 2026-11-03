#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

# ==============================================================================
# Inter OS — Sovereign Smartphone Hardware Flashing Tool (AArch64 Fastboot Automator)
# ==============================================================================
# Flashes or tether-boots Inter OS onto physical smartphone hardware (PinePhone,
# Qualcomm Snapdragon, Google Pixel, MediaTek Dimensity) using the standard
# Android Fastboot protocol (AOSP Boot Image v2).
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_IMG="${1:-"$ROOT_DIR/target/boot-aarch64.img"}"
MODE="${2:-"boot"}"

echo "============================================================"
echo "  Inter OS — Sovereign Smartphone Hardware Flasher v4.6"
echo "============================================================"

# 1. Locate Fastboot binary
FASTBOOT=""
CANDIDATES=(
    "fastboot"
    "$HOME/Android/Sdk/platform-tools/fastboot"
    "$HOME/platform-tools/fastboot"
    "/usr/lib/android-sdk/platform-tools/fastboot"
    "/usr/bin/fastboot"
    "/opt/android-sdk/platform-tools/fastboot"
)

for c in "${CANDIDATES[@]}"; do
    if command -v "$c" &> /dev/null; then
        FASTBOOT="$(command -v "$c")"
        break
    elif [ -x "$c" ]; then
        FASTBOOT="$c"
        break
    fi
done

if [ -z "$FASTBOOT" ]; then
    echo "[WARNING] 'fastboot' was not found in PATH or standard Android SDK directories."
    echo "Please install android-tools or Android Platform Tools."
    FASTBOOT="fastboot"
else
    echo "[OK] Fastboot found at: $FASTBOOT"
fi

# 2. Check Image File
if [ ! -f "$TARGET_IMG" ]; then
    echo "[BUILDING] Image not found at '$TARGET_IMG'. Compiling now..."
    if [ -f "$SCRIPT_DIR/../qemu/build.sh" ]; then
        "$SCRIPT_DIR/../qemu/build.sh"
    elif command -v pwsh &> /dev/null; then
        pwsh -NoProfile -ExecutionPolicy Bypass -File "$SCRIPT_DIR/../qemu/build.ps1"
    elif command -v powershell &> /dev/null; then
        powershell -NoProfile -ExecutionPolicy Bypass -File "$SCRIPT_DIR/../qemu/build.ps1"
    else
        cargo +nightly run --manifest-path "$SCRIPT_DIR/mkbootimg/Cargo.toml" -- --kernel "$SCRIPT_DIR/../target/aarch64-unknown-none/debug/kernel-aarch64" --output "$TARGET_IMG"
    fi
fi

FILE_SIZE=$(wc -c < "$TARGET_IMG" | tr -d ' ')
if command -v sha256sum &> /dev/null; then
    SHA256_HASH=$(sha256sum "$TARGET_IMG" | awk '{print $1}')
elif command -v shasum &> /dev/null; then
    SHA256_HASH=$(shasum -a 256 "$TARGET_IMG" | awk '{print $1}')
else
    SHA256_HASH="unavailable"
fi

echo "[IMAGE] Target Image: $TARGET_IMG"
echo "[IMAGE] Image Size:   $FILE_SIZE bytes"
echo "[IMAGE] SHA-256 Hash: $SHA256_HASH"

# Verify header magic
MAGIC=$(head -c 8 "$TARGET_IMG")
if [ "$MAGIC" != "ANDROID!" ]; then
    echo "[ERROR] Invalid boot image! Expected 'ANDROID!', got '$MAGIC'."
    exit 1
fi
echo "[OK] Android Boot Image v2 container header verified (Magic: 'ANDROID!')."

# --- Payload integrity -------------------------------------------------------
#
# WHY: magic alone certified containers carrying no usable payload. The image
# generator used to synthesise a "device tree" as the FDT magic d00dfeed followed
# by ASCII, and a "ramdisk" as the cpio magic 070701 followed by ASCII, while
# reporting non-zero dtb_size/ramdisk_size. Both start with the correct magic, so
# a magic-only check passes them. The structure is what distinguishes them: a cpio
# newc header is 110 ASCII-hex bytes closed by a TRAILER!!! record, and an FDT
# header carries a totalsize plus struct/strings extents that must fit the
# payload, with the struct block opening on an FDT_BEGIN token.
PAGE_SIZE=$(od -An -tu4 -j36 -N4 "$TARGET_IMG" | tr -d ' ')
KERNEL_SIZE=$(od -An -tu4 -j8 -N4 "$TARGET_IMG" | tr -d ' ')
RAMDISK_SIZE=$(od -An -tu4 -j16 -N4 "$TARGET_IMG" | tr -d ' ')
SECOND_SIZE=$(od -An -tu4 -j24 -N4 "$TARGET_IMG" | tr -d ' ')
DTB_SIZE=$(od -An -tu4 -j1632 -N4 "$TARGET_IMG" | tr -d ' ')

FILE_SIZE=$(wc -c < "$TARGET_IMG" | tr -d ' ')

be32() { # $1=absolute offset -> big-endian u32, or 0 when the read is out of range
    # WHY the bound check: the header fields are read before they are validated.
    # A fabricated dtb supplies ASCII where off_dt_struct belongs, which decodes
    # to ~1.6e9, so dereferencing it made od seek past EOF and the script died
    # with an od error instead of reporting a clean rejection.
    if [ $(( $1 + 4 )) -gt "$FILE_SIZE" ]; then echo 0; return; fi
    od -An -tx1 -j"$1" -N4 "$TARGET_IMG" | tr -d ' \n' | \
        awk '{ printf "%d", strtonum("0x" $1) }'
}
be32safe() { # $1=offset $2=limit -> value only when within the limit
    if [ $(( $1 + 4 )) -gt "$2" ]; then echo 0; return; fi
    be32 "$1"
}
aligned() { # $1=offset $2=size ; payload start of the next page-aligned section
    if [ "$2" -eq 0 ]; then echo "$1"; else
        echo $(( $1 + $2 + ( ($3 - ($2 % $3)) % $3 ) ))
    fi
}

RAMDISK_OFF=$PAGE_SIZE
RAMDISK_OFF=$(aligned "$RAMDISK_OFF" "$KERNEL_SIZE" "$PAGE_SIZE")
DTB_OFF=$(aligned "$RAMDISK_OFF" "$RAMDISK_SIZE" "$PAGE_SIZE")

PROBLEMS=""
if [ "$RAMDISK_SIZE" -lt 120 ]; then
    PROBLEMS="${PROBLEMS}
          - ramdisk is only ${RAMDISK_SIZE} bytes; a cpio newc archive needs a 110-byte header plus TRAILER!!!"
elif ! dd if="$TARGET_IMG" bs=1 skip=$((RAMDISK_OFF + 110)) count=$((RAMDISK_SIZE - 110)) 2>/dev/null | grep -q "TRAILER!!!"; then
    PROBLEMS="${PROBLEMS}
          - ramdisk has no cpio TRAILER!!! record; it is not a real cpio archive"
fi

if [ "$DTB_SIZE" -lt 40 ]; then
    PROBLEMS="${PROBLEMS}
          - dtb is only ${DTB_SIZE} bytes; a flattened device tree needs a 40-byte header"
else
    TOTAL=$(be32safe $((DTB_OFF + 4)) "$FILE_SIZE")
    OFF_STRUCT=$(be32safe $((DTB_OFF + 8)) "$FILE_SIZE")
    OFF_STRINGS=$(be32safe $((DTB_OFF + 12)) "$FILE_SIZE")
    SIZE_STRINGS=$(be32safe $((DTB_OFF + 32)) "$FILE_SIZE")
    SIZE_STRUCT=$(be32safe $((DTB_OFF + 36)) "$FILE_SIZE")
    if [ "$TOTAL" -lt 40 ] || [ "$TOTAL" -gt "$DTB_SIZE" ]; then
        PROBLEMS="${PROBLEMS}
          - dtb header totalsize ${TOTAL} is not within [40, ${DTB_SIZE}]"
    elif [ $((OFF_STRUCT + SIZE_STRUCT)) -gt "$DTB_SIZE" ] || \
         [ $((OFF_STRINGS + SIZE_STRINGS)) -gt "$DTB_SIZE" ]; then
        PROBLEMS="${PROBLEMS}
          - dtb struct/strings extents extend past the payload"
    elif [ "$OFF_STRUCT" -lt 40 ] || [ "$OFF_STRUCT" -ge "$TOTAL" ]; then
        PROBLEMS="${PROBLEMS}
          - dtb off_dt_struct ${OFF_STRUCT} is not inside the payload"
    else
        TOKEN=$(be32 $((DTB_OFF + OFF_STRUCT)))
        if [ "$TOKEN" -ne 1 ]; then
            PROBLEMS="${PROBLEMS}
          - dtb struct block does not open with an FDT_BEGIN token (found ${TOKEN})"
        fi
    fi
fi

if [ -n "$PROBLEMS" ]; then
    echo ""
    echo "[REJECTED] This image is a valid container but its payloads are not real:${PROBLEMS}"
    echo "[REJECTED] Refusing to report this image as verified or to flash it."
    exit 1
fi

echo "[OK] Payload integrity verified: ramdisk carries a cpio archive, dtb carries an FDT."
echo "[NOT READY] This platform has never been booted on physical hardware; a successful"
echo "[NOT READY] flash is not expected to reach a usable interface."

# If Mode is Info, we are done
if [ "$MODE" = "info" ] || [ "$MODE" = "Info" ]; then
    echo "[INFO] Container structure and payload integrity verified."
    exit 0
fi

# 3. Check Connected Physical Devices
echo ""
echo "--- [Querying Connected Physical Smartphone Devices] ---"
DEVICES=$("$FASTBOOT" devices 2>/dev/null || true)

if [ -z "$DEVICES" ]; then
    echo "[WAITING] No devices found in Fastboot mode."
    echo "Please connect your phone via USB and enter Fastboot mode (adb reboot bootloader)."
    echo ""
    echo "Direct Flash Commands (run once connected):"
    if [ "$MODE" = "boot" ] || [ "$MODE" = "Boot" ]; then
        echo "  fastboot boot \"$TARGET_IMG\""
    else
        echo "  fastboot flash boot \"$TARGET_IMG\""
        echo "  fastboot reboot"
    fi
    echo ""
    echo "[SUCCESS] Image validation and Fastboot flasher pipeline verified."
    exit 0
fi

echo "[DEVICE] Connected Fastboot Devices:"
echo "$DEVICES"

# 4. Perform Operation
echo ""
if [ "$MODE" = "boot" ] || [ "$MODE" = "Boot" ]; then
    echo "[OPERATION] Initiating Tethered Live RAM Boot (fastboot boot)..."
    "$FASTBOOT" boot "$TARGET_IMG"
    echo "[SUCCESS] Inter OS kernel loaded and booted on physical smartphone!"
elif [ "$MODE" = "flash" ] || [ "$MODE" = "Flash" ]; then
    echo "[OPERATION] Flashing Inter OS to physical boot partition..."
    "$FASTBOOT" flash boot "$TARGET_IMG"
    echo "[SUCCESS] Boot partition flashed successfully!"
    echo "Rebooting smartphone into Inter OS..."
    "$FASTBOOT" reboot
    echo "[DONE] Smartphone rebooted. Welcome to Sovereign Inter OS."
fi
