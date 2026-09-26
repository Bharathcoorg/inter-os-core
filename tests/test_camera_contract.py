from pathlib import Path

ROOT = Path(__file__).parents[1]
cam = (ROOT / "kernel/src/hal/camera.rs").read_text()
syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text()

assert "CAMERA_MAX_WIDTH: u32 = 1920" in cam
assert "CAMERA_MAX_HEIGHT: u32 = 1080" in cam
assert "pub fn configure(&mut self, width: u32, height: u32)" in cam
assert "pub fn copy_to(&self, out: &mut [u8])" in cam
assert "camera output buffer too small" in cam
assert "match action" in syscall
assert "AutoFocusState::Locked" in syscall
assert "frame.copy_to(out_buf)" in syscall
assert "SyscallResult::InvalidArguments" in syscall
print("M35 camera contract: PASS")
