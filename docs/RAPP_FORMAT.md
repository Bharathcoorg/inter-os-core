# InterOS RAPP Runtime Format v1

The kernel binary RAPP format is the canonical **runtime** package format.

A developer distribution package may wrap this runtime package with UI/resources
and a signed distribution manifest, but the file handed to the kernel package
service must be the canonical binary RAPP payload.

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
- Activation is persisted transactionally through the metadata journal
- Anti-downgrade rejects an older version over an active newer version

## Transaction flow

```text
candidate .rapp
    |
    v
parse header
    |
    v
validate architecture / bounds / digest
    |
    v
check package revocation
    |
    v
check version policy
    |
    v
persist staged -> active transaction
    |
    v
launch isolated process
```

The current v1 binary parser verifies SHA-256 integrity. Ed25519 publisher
signature enforcement remains a separate trust-root layer; unsigned development
packages are therefore not equivalent to production-trusted packages.
