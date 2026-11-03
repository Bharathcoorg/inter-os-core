# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT=Path(__file__).parents[1]
im=(ROOT/"kernel/src/sensor/imu.rs").read_text(encoding="utf-8")
pr=(ROOT/"kernel/src/sensor/proximity.rs").read_text(encoding="utf-8")
sc=(ROOT/"kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
sdk=(ROOT/"sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
assert "pub fn update_gyro" in im and "pub fn sample" in im
assert "distance_mm.min(100)" in pr and "lux.min(100_000)" in pr
assert "out_buf.len() < 32 || out_buf.len() > 512" in sc
assert "pub fn sensor_read(out: &mut [u8])" in sdk
assert "SyscallNumber::SensorRead" in sdk
print("Sensor contract: PASS")
