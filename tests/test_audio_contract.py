from pathlib import Path

ROOT = Path(__file__).parents[1]
pcm = (ROOT / "kernel/src/audio/pcm.rs").read_text()
audio = (ROOT / "kernel/src/audio/mod.rs").read_text()
syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text()

assert "PCM_MAX_FRAMES: usize = 48_000" in pcm
assert "PCM_MAX_BYTES: usize = PCM_MAX_FRAMES * 2 * 2" in pcm
assert "pub fn validate(&self)" in pcm
assert "pub fn encode_le(&self, out: &mut [u8])" in pcm
assert "PCM output buffer too small" in pcm
assert "AUDIO_MAX_BUFFER_FRAMES: usize = 48_000" in audio
assert "configure_stream" in audio and "start_stream" in audio and "stop_stream" in audio
assert "volume_percent: u8" in audio
assert "audio.configure_stream" in syscall
assert "audio.start_stream" in syscall
assert "audio.stop_stream" in syscall
assert "audio.set_volume" in syscall
print("Audio contract: PASS")
