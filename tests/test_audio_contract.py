# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).parents[1]
pcm = (ROOT / "kernel/src/audio/pcm.rs").read_text(encoding="utf-8")
audio = (ROOT / "kernel/src/audio/mod.rs").read_text(encoding="utf-8")
syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")

assert "PCM_MAX_FRAMES: usize = 48_000" in pcm
assert "PCM_MAX_BYTES: usize = PCM_MAX_FRAMES * 2 * 2" in pcm
assert "pub fn validate(&self)" in pcm
assert "pub fn encode_le(&self, out: &mut [u8])" in pcm
assert "PCM output buffer too small" in pcm
assert "AUDIO_MAX_BUFFER_FRAMES: usize = 48_000" in audio
assert "configure_stream" in audio and "start_stream" in audio and "stop_stream" in audio
# WHY `Option<u8>` and not `u8`: the field used to be a `u8` seeded with the
# literal 100, which is a fabricated level - nothing in the audio path ever
# applied it. `AudioManager::play` dispatches to a PIT tone generator with no
# amplitude parameter, `pcm.rs` never reads the manager, and the PCI audio
# driver writes no mixer register. The type now carries the difference between
# "no level has ever been established" (`None`, the boot state, which must stay
# the boot state) and "a level was requested" (`Some`), so no consumer can
# render a confident percentage for a value no hardware produced.
assert "volume_percent: Option<u8>" in audio
assert "volume_percent: None" in audio
assert "audio.configure_stream" in syscall
assert "audio.start_stream" in syscall
assert "audio.stop_stream" in syscall
assert "audio.set_volume" in syscall
print("Audio contract: PASS")
