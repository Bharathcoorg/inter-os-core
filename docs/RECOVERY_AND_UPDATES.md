# Recovery and Secure Updates

InterOS treats system updates as an authenticated transaction rather than an unconditional overwrite.

## Update flow

1. Construct an update manifest containing a monotonically increasing version, payload length, and SHA-256 digest.
2. Reject empty, malformed, truncated, or digest-mismatched payloads.
3. Reject versions that are not newer than the installed version.
4. Verify the canonical manifest with the configured InterOS Ed25519 publisher trust root.
5. Stage the image into the inactive A/B slot.
6. Mark the slot pending and count boot attempts.
7. Confirm the new slot only after the system reaches a healthy userspace state.
8. Roll back to the last active slot after the bounded boot-attempt limit.

The kernel provides the policy and transaction primitives. Flash/slot backends remain platform-specific and must report truthful hardware availability.

## Recovery invariants

- A failed update must never replace the last known-good slot.
- Downgrades are rejected by default.
- Signature or digest failures are fail-closed.
- A pending slot is not considered healthy until explicitly confirmed.
- Recovery decisions are deterministic and bounded.
