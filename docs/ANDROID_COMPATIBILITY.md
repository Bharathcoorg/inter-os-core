# Android Compatibility & Sovereign microG Architecture (Zone 4)

InterOS Core provides a sandboxed, memory-bounded Android compatibility runtime operating within unprivileged userspace (x86_64 Ring 3 / AArch64 EL0 / RISC-V U-Mode) and kernel-mediated Zone 4 boundaries. It allows lightweight Android applications and services to run securely without the bloat, battery drain, or surveillance telemetry of traditional Android Open Source Project (AOSP) or Google Play Services images.

---

## 1. Implemented Subsystems

### 1.1 APK Archive Engine & Streaming Deflate Decompression
- **Header Parsing**: Complies with standard ZIP-32 local and central directory formats up to 4096 entries and 128 MiB per archive member (`kernel/src/compat/android/apk.rs`).
- **Pure `#![no_std]` Deflate Decompressor**: RFC 1951 compliant streaming decompressor (`kernel/src/compat/android/inflate.rs`) utilizing a fixed 32 KiB sliding window buffer with zero heap allocations during decompression. Supports Stored (0) and Deflate (8) compression methods.
- **Fail-Closed Compression Validation**: Unsupported compression methods fail closed with `ApkError::UnsupportedCompression`.
- **Manifest Parser**: Binary XML manifest parser (`AndroidManifest.xml`) extracting package identity, version codes, permissions, and main launch activities without string fabrication.

### 1.2 Bounded Dalvik Bytecode Engine
- **Zero-Copy DEX Slices**: Direct zero-copy slicing of string, type, proto, field, and method pools from `classes.dex` (`kernel/src/compat/android/dex.rs`).
- **Bounded Dalvik Interpreter**: Implements standard format 35c register decoding, arithmetic, control flow (`if-eq`, `if-ne`, `goto`), instance allocation, and method invocations (`kernel/src/compat/android/runtime.rs`).
- **Resource Limits**:
  - `STEP_LIMIT`: 4096 instructions per method execution.
  - `TOTAL_INSTRUCTION_LIMIT`: 1,000,000 instructions per VM instance.
  - `MAX_DALVIK_OBJECTS`: 256 VM-local bounded objects.
  - Maximum recursion call depth bounded to 32 frames to prevent stack exhaustion.
- **Fail-Closed Execution**: Unsupported opcodes and framework effects strictly fail closed with `DalvikVmError::UnsupportedFrameworkApi`.

### 1.3 Sovereign microG Sandbox Engine (`binder.rs`)
InterOS replaces Google Play Services (GMS) with sovereign, local, privacy-preserving implementations:
- **Zero-Allocation AIDL Parcel Engine**: `ParcelReader` and `ParcelWriter` implementing 4-byte natural alignment and UTF-16 character conversions.
- **Sovereign Service Handles**:
  - `HANDLE_SERVICE_MANAGER` (`0`): Enumerate and query active Android services.
  - `HANDLE_MICROG_GMS_BROKER` (`0x1001`): Intercepts `GoogleAuthUtil` / `SignIn` requests, bridging authentication securely to the local cryptographic Identity Vault (`apps/vault`).
  - `HANDLE_MICROG_LOCATION` (`0x1002`): Maps `ILocationManager` to local InterOS GNSS / sensor hardware with strict capability gating (`CAP_LOCATION_READ`), preventing background location leaks.
  - `HANDLE_MICROG_GCM_PUSH` (`0x1003`): Local end-to-end encrypted notification push broker.
  - `HANDLE_MICROG_ADS_ID` (`0x1004`): Returns zeroed advertising identifiers (`00000000-0000-0000-0000-000000000000`) and enforces `isLimitAdTrackingEnabled = true` to eliminate cross-app tracking.

### 1.4 Virtualized Android Sandbox Filesystem (`vfs.rs`)
- **Zone 4 Storage Isolation**: Dedicated virtual directory structure (`/system`, `/system/bin`, `/system/framework`, `/data`, `/data/app`, `/data/data`, `/sdcard`).
- **Security Boundary Enforcement**: Any attempt to access host InterOS paths (`/system/etc/policy.json`, `/system/etc/device_id`, `/packages/`, `/data/audit.log`, or `..` path traversals) is intercepted, rejected with `AndroidFsError::SecurityViolation`, and incremented in security telemetry.

### 1.5 Cross-Zone Intent Bridge (`bridge.rs`)
- **Zero-Trust Translation**: Translates Android Intents into verified sovereign system actions:
  - `android.intent.action.VIEW` + `upi://` / `pay` -> `INTENT_PAY`
  - `android.intent.action.VIEW` + `http://` / `https://` -> `INTENT_BROWSER_URL`
  - `android.intent.action.VIEW` + `geo:` -> `INTENT_MAPS_VIEW`
  - `android.intent.action.DIAL` / `CALL` -> `INTENT_TELEPHONY_DIAL`
  - `android.intent.action.SEND` -> `INTENT_SHARE_PAYLOAD`
- **Path Sanitization**: Rejects all URIs containing `file://`, `content://`, `policy.json`, or directory traversal components.

### 1.6 Bounded DRM/KMS Render Pipeline (`draw.rs` & `window.rs`)
- **Activity Surfaces**: Bounded 720x1320 portrait surfaces created via `DRM_KMS` with 120 pixels reserved for the sovereign status and navigation bars.
- **Area Budgeting**: Bounded to 256 draw commands per list and a cumulative 2-frame area budget (`MAX_FRAME_AREA_PIXELS = 720 * 1320 * 2`) to prevent denial-of-service rasterization.

---

## 2. Platform Truth & Security Commitments

1. **Zero Ambient Authority**: Android workloads receive zero ambient capabilities. All hardware and service accesses require explicit `CapHandle` grants.
2. **Fail-Closed Invariant**: Missing framework methods, unverified APK signatures, or unsupported opcodes report honest refusal (`UnsupportedApi` / `UnsupportedCompression`) rather than fabricating success.
3. **No External Telemetry**: Zero analytics, zero ad beacons, and zero device identifiers cross to third-party endpoints.
