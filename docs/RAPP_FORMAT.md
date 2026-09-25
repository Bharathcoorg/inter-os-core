# InterOS RAPP Runtime Format v1

The kernel binary RAPP format is the canonical **runtime** package format.

## Runtime invariants

- Magic: `RAPP`
- Format version: `1`
- Explicit target architecture
- Package ID and publisher ID
- Semantic version tuple
- Capability declaration
- SHA-256 payload digest
- Bounded payload length
- ELF payload bounds validation
- Package identity must match the requested package ID
- Package architecture must match the running kernel
- Package revocation is checked before activation
- Activation is persisted transactionally
- Anti-downgrade rejects an older version over an active newer version
- Ed25519 signature covers the complete package with the signature field zeroed
- Package-supplied public keys are never trust anchors
- Release builds reject unsigned RAPPs

## Trust-root configuration

Production kernel builds provide:
`INTEROS_TRUSTED_PUBLISHER_KEY_HEX=<64 hex characters>`

The current trusted publisher identity is `interos_official`. The private signing key
belongs only in the external release/signing environment and is never compiled into
the kernel.

Development builds retain an explicit unsigned-package exception for embedded QEMU
development artifacts.

## Transaction flow

```text
candidate .rapp
 -> parse header
 -> validate architecture / bounds / digest
 -> verify publisher trust root + Ed25519 signature
 -> check package revocation
 -> check version / anti-downgrade policy
 -> persist staged -> active
 -> launch isolated process
```

The metadata journal has separate record kinds for package registry and revocation
state, with redundant slots and generation numbers. This prevents one metadata
subsystem from overwriting the other.

The remaining release-engineering step is external signing of final first-party
artifacts and injection of the trusted public key; no private signing key is stored
in the repository.
