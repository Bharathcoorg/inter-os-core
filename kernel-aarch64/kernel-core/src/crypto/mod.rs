//! Cryptographic primitives the metadata journal is built from.
//!
//! Extracted from the x86 kernel's `crypto/` module. Only the two primitives the
//! journal itself needs are here: `record_tag` binds every header field and the
//! whole payload of a journal record with a keyed MAC, and the MAC needs SHA-256.
//!
//! Both are pure computation over bytes, so they moved with no change at all —
//! these two files are byte-identical copies of `kernel/src/crypto/sha256.rs`
//! and `kernel/src/crypto/hmac.rs`, which is the property that makes this crate
//! diffable against the x86 tree.
//!
//! What is deliberately NOT here, and why: the rest of the x86 `crypto/` module
//! cannot move as it stands.
//!
//! * `rng.rs` executes `cpuid`, `rdrand` and `rdtsc` inline
//!   (kernel/src/crypto/rng.rs:30, :45, :68, :85) and reads
//!   `crate::interrupts::get_system_ticks()` (kernel/src/interrupts.rs:154). The
//!   instruction selection is x86-only and the tick counter is the PIT's. Both
//!   need a HAL trait: an entropy source backed by `RNDR` on AArch64, the `seed`
//!   CSR on RISC-V and `rdrand` on x86, plus a monotonic counter backed by the
//!   ARM Generic Timer, the CLINT `mtime`, and the PIT respectively.
//! * `keystore.rs` calls `crate::storage::persistent::{load_latest_kind,
//!   commit_kind, set_journal_auth_key}` (kernel/src/crypto/keystore.rs:187,
//!   :213, :442) — those are in this crate now — but it also calls
//!   `crate::crypto::get_random_bytes` (:196, :270) and `crate::serial_println!`
//!   (:395 and five more), so it waits on the entropy HAL and the console seam.
//! * `ed25519.rs` is a wrapper over the `ed25519-dalek` crate and is portable as
//!   it is, but it is a signature scheme, not a hash, and it is not part of this
//!   subsystem.

pub mod hmac;
pub mod sha256;

pub use hmac::HmacSha256;
pub use sha256::{Sha256, SHA256_BLOCK_SIZE, SHA256_DIGEST_LENGTH};
