# Android compatibility boundary

InterOS currently contains an isolated Android compatibility research runtime. The implementation includes APK/ZIP structure parsing, binary manifest parsing, DEX parsing and bytecode auditing, a bounded subset Dalvik interpreter, a constrained framework-call bridge, Android activity/window lifecycle records, per-app data paths, and capability-mediated Binder-style transactions.

## Implemented components

- APK container structure and manifest parsing.
- DEX structural checks and an explicitly limited opcode interpreter.
- Per-method instruction limits and a cumulative per-VM execution budget.
- Bounded Android framework log capture and explicit rejection of unsupported framework APIs.
- Per-app runtime state, guest compositor surfaces, activity lifecycle states, and cleanup when surface allocation fails.
- Capability checks for mediated Binder-style IPC; Android calls do not receive ambient host authority.

## Compatibility limitations

The current APK signature verifier intentionally fails closed. It is **not yet** a complete Android APK Signature Scheme v2/v3 cryptographic verifier, so a signing-block marker or certificate filename is not accepted as proof of authenticity. Arbitrary third-party APK installation and launch are therefore not supported yet.

This runtime is not a full Android platform. ART/JIT, broad DEX opcode coverage, the complete Android framework and system services, the full Binder driver/protocol, Google Play Services, Android Linux syscall ABI compatibility, vendor HAL compatibility, and production APK signer/certificate-chain verification remain unsupported.

## Release acceptance

Do not describe Android compatibility as production-ready until representative signed APKs can be installed and verified against an explicit trust policy, launched as actual guest workloads, render through the InterOS compositor, receive guest input, use implemented framework services, preserve sandboxed app data, and terminate cleanly under QEMU. Security, resource-limit, repeated-lifecycle, and negative tests must pass first. Physical-device support remains unverified until real hardware testing is performed.

The public compatibility bridge should grow through tested API families mapped to real InterOS services. Unsupported calls must return explicit errors rather than synthetic success. The private optimized runtime may improve resource use, but must not weaken the public security boundary.
