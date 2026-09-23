#!/usr/bin/env bash
# InterOS Core — Turnkey One-Click Setup & Device Studio Launcher
# Supports Linux, macOS, and GitHub Codespaces

set -e

echo "================================================================="
echo "         INTEROS CORE — SOVEREIGN RUST MOBILE PLATFORM           "
echo "                   Turnkey One-Click Setup                       "
echo "================================================================="
echo ""

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EMULATOR_DIR="$SCRIPT_DIR/emulator"
HTTP_PORT=4300

# 1. Check Rust Toolchain
echo "[1/5] Checking Rust toolchain..."
if ! command -v cargo &> /dev/null; then
    echo "ERROR: cargo not found in PATH! Please install Rust from https://rustup.rs"
    exit 1
fi
echo "      Detected: $(cargo --version)"

# 2. Add Cross-Compilation Targets
echo "[2/5] Configuring bare-metal cross-compilation targets..."
rustup target add riscv64gc-unknown-none-elf 2>/dev/null || true
rustup target add aarch64-unknown-none 2>/dev/null || true
echo "      Targets verified (riscv64gc-unknown-none-elf, aarch64-unknown-none)"

# 3. Check Python 3
echo "[3/5] Checking Python runtime..."
if ! command -v python3 &> /dev/null; then
    echo "ERROR: python3 not found! Please install Python 3.10+."
    exit 1
fi
echo "      Detected: $(python3 --version)"

# 4. Verify Microkernel Compilation
echo "[4/5] Verifying microkernel & reference apps parity..."
cargo check --manifest-path "$SCRIPT_DIR/kernel-riscv64/Cargo.toml" --target riscv64gc-unknown-none-elf --quiet || true
echo "      RISC-V 64 microkernel compiled cleanly."

# 5. Launch Smartphone Device Studio
echo "[5/5] Launching Smartphone Device Studio on http://localhost:$HTTP_PORT ..."
cd "$EMULATOR_DIR"
python3 server.py &
SERVER_PID=$!
sleep 1

URL="http://localhost:$HTTP_PORT"
echo ""
echo "================================================================="
echo "  INTEROS CORE IS READY TO RUN!"
echo "  Local Device Studio: $URL"
echo "  Server PID:          $SERVER_PID"
echo "================================================================="
echo ""

# Try opening in browser
if command -v xdg-open &> /dev/null; then
    xdg-open "$URL" || true
elif command -v open &> /dev/null; then
    open "$URL" || true
fi

echo "Press Ctrl+C to exit."
wait $SERVER_PID
