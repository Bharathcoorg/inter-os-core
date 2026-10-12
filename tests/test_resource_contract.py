from pathlib import Path
ROOT=Path(__file__).parents[1]
b=(ROOT/"kernel/src/resource/battery.rs").read_text(encoding="utf-8"); t=(ROOT/"kernel/src/resource/thermal.rs").read_text(encoding="utf-8"); s=(ROOT/"kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
assert "BATTERY_JSON_MAX: usize = 256" in b
assert "design_capacity_mah == 0" in b
assert "THERMAL_MIN_C: i32 = -40" in t and "THERMAL_MAX_C: i32 = 150" in t
assert "THERMAL_JSON_MAX: usize = 256" in t
assert "temp_c.clamp(THERMAL_MIN_C, THERMAL_MAX_C)" in t
assert "out_buf.len() < crate::resource::BATTERY_JSON_MAX" in s
assert "out_buf.len() < crate::resource::THERMAL_JSON_MAX" in s
print("Resource telemetry contract: PASS")
