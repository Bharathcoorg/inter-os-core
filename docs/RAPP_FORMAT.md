# InterOS RAPP Binary Package Specification

The **RAPP** (Rust Application Package) binary format is the native, deterministic execution package format for InterOS Core (`kernel/src/package/rapp.rs`).

---

## 1. Binary Layout & Header Structure

A `.rapp` file is a contiguous, non-archive binary structure consisting of a fixed header, a capability ID table, and an embedded native ELF executable payload:

```text
+-------------------------------------------------------------+
| RAPP Header (180 bytes / 0xB4)                              |
| - Magic: b"RAPP" (4 bytes)                                  |
| - Format Version: 2 (2 bytes, LE u16)                       |
| - Architecture: 0=x86_64, 1=aarch64, 2=riscv64 (2B, LE u16) |
| - Package ID: UTF-8, null-padded (32 bytes)                 |
| - Publisher Ed25519 Public Key (32 bytes)                   |
| - Semantic Version: (major, minor, patch) (6 bytes, 3x u16) |
| - Capability Count: N (2 bytes, LE u16)                     |
| - Payload SHA-256 Digest (32 bytes)                         |
| - Ed25519 Signature (64 bytes)                              |
| - Payload Length (4 bytes, LE u32)                          |
+-------------------------------------------------------------+
| Capability IDs: N x LE u16 (2 * N bytes)                    |
+-------------------------------------------------------------+
| Executable ELF Payload (Payload Length bytes)               |
+-------------------------------------------------------------+
```

### Exact Field Offsets (RAPP v2, v3 & v4)

| Byte Range | Length | Type | Field Description |
| :--- | :--- | :--- | :--- |
| `0x00 - 0x03` | 4 bytes | `[u8; 4]` | Magic identifier: ASCII `b"RAPP"` (`0x52, 0x41, 0x50, 0x50`) |
| `0x04 - 0x05` | 2 bytes | `u16` (LE) | Format version: `2` (standard), `3` (dependency extension) or `4` (dependency and intent extension) |
| `0x06 - 0x07` | 2 bytes | `u16` (LE) | Target CPU architecture (`0` = x86_64, `1` = AArch64, `2` = RISC-V 64) |
| `0x08 - 0x27` | 32 bytes | `[u8; 32]` | Package identifier (UTF-8, null-padded, max 32 chars) |
| `0x28 - 0x47` | 32 bytes | `[u8; 32]` | Publisher Ed25519 public key |
| `0x48 - 0x4D` | 6 bytes | `[u16; 3]` | Semantic version tuple: `(major, minor, patch)` |
| `0x4E - 0x4F` | 2 bytes | `u16` (LE) | Declared capability count `N` (maximum 1024) |
| `0x50 - 0x6F` | 32 bytes | `[u8; 32]` | SHA-256 cryptographic digest of the embedded ELF payload |
| `0x70 - 0xAF` | 64 bytes | `[u8; 64]` | Ed25519 digital signature over canonical signing message |
| `0xB0 - 0xB3` | 4 bytes | `u32` (LE) | Embedded ELF payload length in bytes |
| `0xB4 - 0xB7` | 4 bytes | `u32` (LE) | *(RAPP v3)* Dependency table length; *(RAPP v4)* Signed section length |

### Payload & Table Offsets

- **RAPP v2 Header**: 180 bytes (`0xB4`).
  - Capabilities table: `0xB4 .. 0xB4 + 2*N` (Array of declared capability IDs, `u16` LE).
  - ELF executable binary payload: `0xB4 + 2*N .. 0xB4 + 2*N + payload_len`.
  - Exact total length: `0xB4 + 2*N + payload_len`.
- **RAPP v3 Header**: 184 bytes (`0xB8`).
  - Capabilities table: `0xB8 .. 0xB8 + 2*N`.
  - Dependency table: `0xB8 + 2*N .. 0xB8 + 2*N + dependency_len`.
    - `dep_count`: 2 bytes (`u16` LE, max 64).
    - Each dependency entry: `id_len` (1B u8) + `package_id` (`id_len` bytes UTF-8) + `kind` (1B: 0=Exact, 1=Minimum, 2=Major) + `major` (2B) + `minor` (2B) + `patch` (2B).
  - ELF executable binary payload: follows immediately after dependency table for `payload_len` bytes.
  - Exact total length: `0xB8 + 2*N + dependency_len + payload_len`.
- **RAPP v4 Header**: 184 bytes (`0xB8`) — the same header as v3.
  - Capabilities table: `0xB8 .. 0xB8 + 2*N`.
  - Signed section: `0xB8 + 2*N .. 0xB8 + 2*N + section_len`.
    - `dep_len`: 4 bytes (`u32` LE) — the length of the dependency table that follows.
    - `intent_len`: 4 bytes (`u32` LE) — the length of the intent table that follows the dependency table.
    - Dependency table: `dep_len` bytes, encoded exactly as in v3.
    - Intent table: `intent_len` bytes.
      - `intent_count`: 2 bytes (`u16` LE, max 64).
      - Each intent entry: `kind` (1B: 0=mime, 1=scheme, 2=action) + `value_len` (1B u8, 1..=96) + `value` (`value_len` bytes UTF-8).
  - ELF executable binary payload: follows immediately after the signed section for `payload_len` bytes.
  - Exact total length: `0xB8 + 2*N + section_len + payload_len`.

### The manifest extension: declared handlers (RAPP v4)

A package declares what it can handle in its `manifest.json`:

```json
{
  "id": "org.interos.gallery",
  "name": "Gallery",
  "version": "1.0.0",
  "permissions": ["CAP_LOGGING", "CAP_CONTENT_INDEX", "CAP_CONTENT_INTENT"],
  "intents": {
    "mime":   ["image/png", "video/mp4"],
    "scheme": ["http", "https"],
    "action": ["org.interos.action.set_wallpaper"]
  }
}
```

`interos package <name>` writes a **RAPP v4** package when the manifest carries an
`intents` block, and a RAPP v2 package otherwise. The three keys are the three
kinds of thing a package can declare a handler for:

| Key | Wire `kind` | Example | What it means |
| :--- | :--- | :--- | :--- |
| `mime` | `0` | `image/png` | The package opens documents of that media type |
| `scheme` | `1` | `https` | The package opens URIs of that scheme |
| `action` | `2` | `org.interos.action.set_wallpaper` | The package performs that named action |

The declarations are inside the **signed** section of the package, so a claim a
package makes about what it handles cannot be edited after signing. The
kernel's intent registry is built from these tables: a package is registered
when it is installed and withdrawn when it is uninstalled, and the registry
answers "who handles this?" only from what is installed.

A media type that no installed package declares has **no handler**. The
platform refuses such an intent and names nothing, rather than offering a
substitute.

---

## 2. Cryptographic Attestation & Verification Pipeline

When `parse_rapp(data: &[u8])` executes in the microkernel:

1. **Magic & Format Check**: Asserts `data[0..4] == b"RAPP"` and format version equals `2` (or `3`, or `4`).
2. **Architecture Match**: Rejects binaries if the target architecture does not match the active kernel architecture.
3. **Exact Buffer Bounds**: Total slice length must strictly equal `0xB4 + 2*N + payload_len` (for v2), `0xB8 + 2*N + dependency_len + payload_len` (for v3), or `0xB8 + 2*N + section_len + payload_len` (for v4). Packages with trailing garbage, truncated payloads, or length mismatches are rejected with `RappError::BufferTooShort`, `RappError::PayloadOverflow`, or `RappError::InvalidPayload`.
4. **Payload Digest Verification**: Computes the SHA-256 digest over the ELF payload bytes and asserts exact equality with the header `payload_digest`.
5. **Trust Root Evaluation**: Checks the publisher's public key against the kernel's active trust store (`REVOCATION_REGISTRY`). Untrusted or revoked keys return `RappError::PublisherUntrusted` or `RappError::PublisherRevoked`.
6. **Canonical Signature Verification**:
   - The signing message is reconstructed by copying the header with the 64-byte signature field (`0x70..0xB0`) set to zero, followed by the capability array, followed by the signed section (the dependency table for v3, the dependency and intent tables for v4), followed by the ELF payload bytes.
   - The signature is verified using `ed25519_dalek::Verifier`. Packages with zeroed, corrupt, or invalid signatures are rejected with `RappError::SignatureInvalid`.

---

## 3. Standard Capability Identifiers

Declared capability IDs map into per-process capability token table rights and object scoping:

| Capability ID | Capability Type | Default Rights | Object Scope / Target | Description |
| :--- | :--- | :--- | :--- | :--- |
| `1` | `SerialOutput` | `WRITE` | `0` (Global) | Serial logging and COM port character output |
| `2` | `FrameBufferDirect` | `WRITE` | `0` (Global) | Direct framebuffer pixel drawing and display hardware presentation |
| `3` | `TimerTickRead` | `READ` | `0` (Global) | High-precision system timer tick polling |
| `4` | `HardwarePortIo` | `READ \| WRITE` | `0` (Global) | Direct port I/O hardware bus access |
| `5` | `IpcSend` | `WRITE` | IPC Target Endpoint | Send messages through dedicated IPC channel |
| `6` | `IpcReceive` | `READ` | IPC Target Endpoint | Receive messages through dedicated IPC channel |
| `7` | `ProcessSpawn` | `INVOKE` | `0` (Global) | Spawn child user processes and containers |
| `8` | `ProcessKill` | `INVOKE` | `0` (Global) | Terminate owned child processes |
| `9` | `FileSystemRead` | `READ` | `0` (Sandboxed) | Read files within sandboxed VFS mount |
| `10` | `FileSystemWrite` | `WRITE` | `0` (Sandboxed) | Write/persist files within sandboxed VFS mount |
| `11` | `StorageRawBlock` | `READ \| WRITE` | `0` (Global) | Direct block-level ATA/NVMe storage operations |
| `12` | `NetworkSocket` | `WRITE` | `0` (Global) | Create UDP/TCP transport sockets |
| `13` | `NetworkSend` | `WRITE` | `0` (Global) | Transmit network packets |
| `14` | `NetworkReceive` | `READ` | `0` (Global) | Receive network packets |
| `15` | `ShmProducer` | `WRITE` | `0` (Global) | Shared memory ring-buffer producer |
| `16` | `ShmConsumer` | `READ` | `0` (Global) | Shared memory ring-buffer consumer |
| `17` | `EventPoll` | `READ \| INVOKE` | `0` (Global) | Poll input and event queues |
| `18` | `SystemControl` | `INVOKE` | `0` (Global) | Global system administration and power management |
| `19` | `SystemControl` | `INVOKE` | `SERVICE_POWER_CONTROL_OBJECT` | Scoped power governor and battery telemetry |
| `20` | `SystemControl` | `INVOKE` | `SERVICE_TELEPHONY_CONTROL_OBJECT` | Scoped cellular modem, SMS, and dialer control |
| `21` | `SystemControl` | `INVOKE` | `SERVICE_AUDIO_CONTROL_OBJECT` | Scoped hardware audio synthesis and playback control |
| `22` | `SystemControl` | `INVOKE` | `SERVICE_CAMERA_CONTROL_OBJECT` | Scoped camera sensor frame capture |
| `23` | `SystemControl` | `INVOKE` | `SERVICE_SENSOR_CONTROL_OBJECT` | Scoped IMU, accelerometer, and sensor reading |

### Capabilities that RAPP v2 does not define

These are called out explicitly so a publisher does not assume an ABI entry is
merely undocumented:

- **No hardware keystore, enclave, or secure-element capability exists.** Secret
  material for a package is persisted in that package's sandboxed VFS mount,
  which the kernel's storage vault encrypts at rest. That is ordinary
  `FileSystemWrite` (id `10`) to the package, not a separate authority, so a
  manifest cannot request enclave access and must not claim to have it.
- **No device-wide administration is grantable to a package.** Id `18` is
  reserved for the system supervisor: the kernel mints it only for a publisher
  key the trust store records as a system publisher, and the runtime gate
  additionally requires a kernel-assigned privileged process identity. Until a
  trusted provisioning path can install such a publisher key, no package can
  obtain id `18`, and any manifest that declares it is refused at install and at
  launch.

An alias or template in the tooling that resolves a name to one of these
capabilities must resolve it to the identifier that actually delivers what the
name promises. `tools/interos.py` enforces this with
`ALIAS_FORBIDDEN_CAPABILITY_IDS`, and `tests/test_rapp_capability_abi.py` pins
it against this table.
| `24` | `RealtimeClockRead` | `READ` | `0` (Global) | Read wall-clock hardware real-time clock (RTC) |
| `25` | `ContentIndexRead` | `READ` | `0` (Global) | Read the shared document index and the intent registry |
| `26` | `ContentIntentDispatch` | `INVOKE` | `0` (Global) | Dispatch an intent to the package that declared a handler for it |
| `28` | `AudioStream` | `READ \| WRITE` | `0` (Global) | Open, feed, observe and close the kernel's AC'97 PCM output stream |

Values outside `1..=28` (such as `0` or `29+`) are strictly unknown and rejected during package verification (`RappError::UnknownCapability`).

**Why the content layer has its own two identifiers.** `ContentIndexRead` is
not `FileSystemRead`: the index *describes* documents — their paths, lengths,
media types and identities — and reading a description is not reading a file.
An app that may browse the shared area's catalogue does not thereby gain the
ability to open the documents in it. `ContentIntentDispatch` is not
`ProcessSpawn`: spawning a process is authority over the device's task table,
while dispatching an intent is authority over one named relationship — "the
package that declared this media type" — which the kernel resolves from the
signed manifests.

---

## 4. Package Lifecycle in Userspace

1. **Scaffolding & Packaging**: Developers scaffold projects via `interos new` and bundle binaries via `interos package <name>`.
2. **Signing**: Packages are signed with the publisher's Ed25519 private key via `interos sign <package> --key <key_file>`.
3. **Trust Provisioning**: The OS carries no publisher trust root of its own. A publisher becomes trusted only when the owner of the device installs its Ed25519 public key into the trust store, and that record is journalled so it survives a reboot. On a QEMU developer guest the owner provisions a root over the COM1 console with `RAPP_TRUST <public key hex> <publisher>`, which the `interos trust <package.rapp>` command issues.

   Platform-authority publisher identities are an explicit allowlist: the exact
   strings `system` and `platform`, plus the delimited namespace `org.interos.`.
   The trailing dot is part of the namespace, so `org.interos-evil` and
   `org.interosX` are not platform publishers.

   **Current state of provisioning.** There is no firmware-anchored provisioning path and no userspace syscall that grants a publisher root, so the COM1 console is the only way a root reaches the store. That console is not an authenticated channel, which is why it may install an ordinary publisher but refuses a platform-authority identity. On a device with no installed platform root, **no publisher is a platform publisher and capability 18 cannot be granted**. This is deliberate fail-closed behaviour and it is the current state, not a pending detail: until an authenticated provisioning path is implemented, capability 18 must be treated as reserved rather than unavailable.
4. **Installation & Storage**: `interos install <package>` transfers the signed bytes to the guest over QEMU COM1, verifies integrity and signatures inside Ring 0 against the active trust store, and commits the package blob into persistent storage.
5. **Execution**: The launcher or PID 1 spawns an isolated Ring-3 child process with dedicated page tables (`W^X` enforced) and passes minted capability tokens.

---

## 5. Truthful Media & Codec Support Matrix

In accordance with the **Platform Truth Contract**, first-party packages and runtime manifests must advertise only codecs and containers that are genuinely decoded by native software or verified hardware on the platform. Unsupported codecs must fail closed, return typed errors, or report unavailable rather than synthesizing frames or faking telemetry.

Evidence levels used below:

- **Source-only** — the code path exists but no test or QEMU run has exercised it.
- **Unit-tested** — exercised by `cargo test` or a contract test in `tests/`.
- **QEMU-verified** — exercised in the reference QEMU x86_64 build through the real app-launch path, with serial log and/or scanout screenshot evidence.

| Category | Media Type / Container | Status | Handler Package | Evidence | Notes & Constraints |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Audio** | `audio/wav` | **Supported** | `org.interos.music` | Unit-tested | RIFF/WAVE PCM (8, 16, 24, 32-bit; mono/stereo). Resampled to AC'97 48 kHz stereo. The index emits `audio/wav`; `audio/x-wav` is not emitted by the platform, so it is not advertised. |
| **Audio** | `audio/flac` | **Supported** | `org.interos.music` | Unit-tested | FLAC 16/24-bit lossless streaming. Duration read from STREAMINFO block. |
| **Audio** | `audio/mpeg` | **Supported** | `org.interos.music` | Unit-tested | MPEG-1 Audio Layer III (MP3), 32/44.1/48 kHz. Stream duration honestly reported as unknown (`--:--`). |
| **Audio** | `audio/ogg`, `audio/aac`, `audio/mp4` | **Refused** | *None* | Source-only | No Ogg Vorbis, AAC, or MP4 audio decoder in tree. Pruned from manifest intents. The importer still lists `.ogg`, `.m4a`, `.aac`, and `.opus` files, but playback refuses them rather than faking output. |
| **Video** | `video/avi`, `video/x-msvideo` | **Supported** | `org.interos.video_player` | QEMU-verified, unit-tested | RIFF AVI carrying an MJPEG (`MJPG`/`JPEG`) video stream. Verified in the reference QEMU build by uploading `apps/video_player/tests/fixtures/mjpeg_5fps_audio.avi` (6394 bytes, 64x48, 10 frames, 2 s) to `/storage/videos/audio_tone.avi` and playing it; the serial log records the open, the audio-stream notice, and end of stream. Other codecs are refused with `UnsupportedCodec`. |
| **Video** | `video/x-motion-jpeg` | **Supported** | `org.interos.video_player` | Unit-tested | Motion JPEG single-frame/stream parsing and baseline JPEG decode. The AVI path above also decodes MJPEG payloads through the same decoder. |
| **Video** | `video/x-ppm` | **Supported** | `org.interos.video_player` | Unit-tested | Uncompressed PPM P6 frames in `/storage/videos`. The content index itself classifies P6 as `image/x-portable-pixmap`; File Manager maps `/storage/videos/*.ppm` to `video/x-ppm` while retaining the index provenance. Frame decoding is covered by `test_ppm_p6_frame_decode`; no QEMU run has opened a PPM through the player. |
| **Video** | `video/x-raw` | **Supported** | `org.interos.video_player` | Source-only | Raw RGB24/RGBA frames with a header, opened by suffix from `/storage/videos`. No fixture or test covers this path yet. |
| **Video** | `video/mp4`, `video/webm`, `video/quicktime` | **Refused** | *None* | Unit-tested | H.264, VP9, and AV1 decode requires physical silicon. Refused with exact contract string: `"No hardware H.264 decoder bound. Software decode unavailable."` The gallery does not advertise these types: it has no video decoder and its handoff refuses when the gallery itself is the first registered handler. |
| **Video Audio** | Embedded AVI Audio | **Unavailable** | `org.interos.video_player` | QEMU-verified (log) | Audio streams inside AVI files are detected but not decoded. The player logs `"[VIDEO PLAYER] AVI has an audio stream; audio playback is unavailable and is not claimed."` and shows `"Audio stream: unavailable"` while the stream is open. |
| **Images** | `image/png`, `image/jpeg`, `image/gif` | **Supported** | `org.interos.gallery` | Unit-tested | Decoded via native zero-heap image decoders. |
| **Images** | `image/webp` | **Supported** | `org.interos.gallery` | Unit-tested | Lossless (VP8L) fully supported; lossy (VP8 keyframe) decoder implemented in standard library with fixture test coverage (`vp8_fixtures.rs`). |
| **Images** | `image/svg+xml` | **Supported** | `org.interos.gallery`, `org.interos.browser` | Unit-tested | Decoded via native zero-heap bounded SVG subset renderer. |
| **Images** | `image/bmp`, `image/qoi`, `image/x-portable-pixmap` | **Supported** | `org.interos.gallery` | Unit-tested | Fully supported native raster formats. |

AVI playback is a sequential, forward-only stream. The player cannot seek an AVI: the scrubber and seek target log `"[VIDEO PLAYER] A sequential AVI cannot seek; position unchanged."` and leave the position alone. Pacing comes from the `avih`/`strh` timing; when decoding falls behind, frames are dropped rather than played in slow motion. End of stream closes the file and logs `"[VIDEO PLAYER] AVI end of stream; the file was closed."`. Sub-second durations display as `--:--` because the player reports whole seconds.

