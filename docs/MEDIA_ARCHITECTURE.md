# InterOS Core — Media Architecture & Audio/Video Synchronization

## 1. Userspace Video Player Pipeline (`apps/video_player/`)

The InterOS native video player is a `#![no_std]` and `#![no_main]` userspace application operating with zero heap allocation in interactive playback and rendering loops.

### Container & Codec Support
1. **RIFF AVI (`video/avi`, `video/x-msvideo`)**:
   - Parsed by `apps/video_player/src/avi.rs`.
   - Traverses RIFF chunks, parses `hdrl` (`avih`, `strh`, `strf`) and streams `movi` payloads.
   - Demuxes video chunks (`XXdc` / `XXdb`) and audio chunks (`XXwb`).
2. **Motion JPEG (`video/x-motion-jpeg`, FourCC `MJPG`/`JPEG`)**:
   - Decoded via pure-software baseline JPEG decoder in `sdk/rust/interos-std/src/image.rs:decode_jpeg()`.
   - Decodes into a static pre-allocated `.bss` frame buffer (`AVI_RGBA_BUF`), then rendered into the 680×382 canvas.
3. **PPM P6 (`video/x-ppm`)**:
   - Uncompressed 24-bit RGB frames parsed directly from `/storage/videos`.
4. **RAW RGB24 / RGBA32 (`video/x-raw`)**:
   - Fixed-dimension uncompressed frames.
5. **VP8 Keyframes (`image/webp`, `video/x-vp8`)**:
   - Software-decoded via `sdk/rust/interos-std/src/vp8.rs` (RFC 6386 zero-heap decoder).

### Platform Truth Contract Compliance
- **H.264 / VP9 / AV1**: In strict adherence to the Platform Truth Contract, hardware decode is never synthesized or mocked without physical silicon.
- Attempting to play H.264 video cleanly displays the exact platform contract string:
  `"No hardware H.264 decoder bound. Software decode unavailable."`

## 2. Audio/Video Clock Synchronization

### Clock Domains
- **Video Clock**: Derived from system monotonic tick timer (`std::get_ticks()`, 1000 Hz).
- **Audio Clock (Master PTS)**: Derived from the AC'97 hardware audio controller ring buffer (`kernel/src/audio/ac97.rs`). The audio hardware consumes 48 kHz 16-bit stereo PCM samples:
  $$\text{Audio PTS (ms)} = \frac{\text{samples\_consumed}}{48000 \times 2} \times 1000$$

### Presentation Time Synchronization
- When video frames lead the Audio PTS by more than $20\text{ ms}$, the render thread yields until the presentation window aligns.
- When video decoding falls behind the Audio PTS by $\ge 1\text{ frame interval}$, the frame decode is skipped (`skipped += 1`) to preserve lip-sync without causing slow-motion playback.
