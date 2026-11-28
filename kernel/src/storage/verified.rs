// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Cryptographic Merkle Tree & Verified Storage Layer
//!
//! Enforces dm-verity inspired integrity verification on system partitions.
//! Any block read from disk or rootfs must have its cryptographic SHA-256
//! digest attested against expected Merkle root digests before it is trusted.
//!
//! WHY this module also owns the boot integrity profile: the kernel-created
//! system inventory is a SOFTWARE-TRUSTED prototype boundary -- the anchor is
//! the kernel image, not a ROM, a secure element or a measured-boot register.
//! A profile that says "verified" therefore has to mean exactly one thing:
//! every required protected object authenticated against that anchor, or the
//! boot refused. The profile is what makes that promise enforceable, so the
//! policy, the activation and the refusal live together here.

use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::drivers::SECTOR_SIZE;
use crate::storage::block::BlockDevice;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use spin::Mutex;

/// Verified block size in bytes (matches standard hardware sector size).
pub const VERIFIED_BLOCK_SIZE: usize = SECTOR_SIZE;

/// Maximum Merkle branch proof depth (supports partitions up to 2^16 = 65,536 blocks = 32 MiB).
pub const MAX_MERKLE_PROOF_DEPTH: usize = 16;

/// Verification and storage error states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    IntegrityViolation,
    BlockOutOfRange,
    DeviceError,
    TamperedData,
    GeometryMismatch,
    UnauthenticatedRoot,
}

/// Cryptographically attested block descriptor.
#[derive(Debug, Clone)]
pub struct VerifiedBlock {
    pub lba: u32,
    pub expected_hash: [u8; SHA256_DIGEST_LENGTH],
}

/// In-kernel Merkle rootfs verifier.
pub struct MerkleVerifier {
    root_digest: [u8; SHA256_DIGEST_LENGTH],
}

impl MerkleVerifier {
    /// Creates a new verifier anchored by an authorized Merkle root hash.
    pub const fn new(root_digest: [u8; SHA256_DIGEST_LENGTH]) -> Self {
        Self { root_digest }
    }

    /// Verifies a data block against its expected SHA-256 digest.
    pub fn verify_block(data: &[u8], expected_digest: &[u8; SHA256_DIGEST_LENGTH]) -> Result<(), StorageError> {
        let actual_digest = Sha256::digest(data);
        if &actual_digest == expected_digest {
            Ok(())
        } else {
            Err(StorageError::IntegrityViolation)
        }
    }

    /// Returns the active Merkle root digest.
    pub fn root_digest(&self) -> &[u8; SHA256_DIGEST_LENGTH] {
        &self.root_digest
    }

    /// Computes parent hash of two child Merkle nodes: SHA256(left || right).
    pub fn combine_hashes(
        left: &[u8; SHA256_DIGEST_LENGTH],
        right: &[u8; SHA256_DIGEST_LENGTH],
    ) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(left);
        hasher.update(right);
        hasher.finalize()
    }

    /// Verifies a cryptographic Merkle branch proof from a leaf block to the anchor root digest.
    pub fn verify_branch_proof(
        &self,
        leaf_data: &[u8],
        leaf_index: usize,
        audit_path: &[[u8; SHA256_DIGEST_LENGTH]],
    ) -> Result<(), StorageError> {
        let mut current_hash = Sha256::digest(leaf_data);
        let mut idx = leaf_index;
        for sibling in audit_path {
            if idx % 2 == 0 {
                current_hash = Self::combine_hashes(&current_hash, sibling);
            } else {
                current_hash = Self::combine_hashes(sibling, &current_hash);
            }
            idx /= 2;
        }
        if current_hash == self.root_digest {
            Ok(())
        } else {
            Err(StorageError::IntegrityViolation)
        }
    }

    /// Validates an in-memory block read with bounds and proof verification.
    pub fn verify_authenticated_read(
        &self,
        block_index: usize,
        total_blocks: usize,
        block_data: &[u8],
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
    ) -> Result<(), StorageError> {
        if block_index >= total_blocks {
            return Err(StorageError::BlockOutOfRange);
        }
        if proof.len() > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }
        if block_data.len() != VERIFIED_BLOCK_SIZE {
            return Err(StorageError::IntegrityViolation);
        }
        self.verify_branch_proof(block_data, block_index, proof)
    }
}

// ---------------------------------------------------------------------------
// Boot integrity profile
// ---------------------------------------------------------------------------

/// The profile a boot runs under.
///
/// WHY two profiles and not one: this device has no hardware trust anchor, so
/// a single profile would have to say "verified" while meaning "the kernel
/// image says so". Two profiles let the kernel say exactly what happened:
/// `Verified` refuses to bring the protected objects up when one of them fails
/// to authenticate, and `UnverifiedDevelopment` comes up anyway and reports the
/// failure. Neither profile is hardware secure boot, and the development
/// profile must never be reported as verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPolicy {
    /// Protected system objects must authenticate; a failure refuses activation.
    Verified,
    /// Protected system objects are best effort; failures are reported and the
    /// boot continues.
    UnverifiedDevelopment,
}

/// The token that names the `Verified` profile in the build and on the device.
pub const BOOT_POLICY_VERIFIED_TOKEN: &str = "verified";

/// The token that names the `UnverifiedDevelopment` profile.
pub const BOOT_POLICY_UNVERIFIED_DEVELOPMENT_TOKEN: &str = "unverified-development";

/// The profile this kernel image was built to run under.
///
/// WHY it is a compile-time constant: an image that ships the verified profile
/// must not be talked out of it by anything the device can write. The build
/// script rejects every other value before the kernel is compiled, so this
/// token is always one of the two profiles.
pub const BUILD_DEFAULT_BOOT_POLICY_TOKEN: &str = env!("INTEROS_BOOT_POLICY");

impl BootPolicy {
    /// The token this profile is named by.
    pub const fn token(self) -> &'static str {
        match self {
            BootPolicy::Verified => BOOT_POLICY_VERIFIED_TOKEN,
            BootPolicy::UnverifiedDevelopment => BOOT_POLICY_UNVERIFIED_DEVELOPMENT_TOKEN,
        }
    }

    /// Whether this profile refuses activation when authentication fails.
    pub const fn refuses_activation_on_failure(self) -> bool {
        matches!(self, BootPolicy::Verified)
    }

    /// Whether this profile is the verified one.
    ///
    /// WHY the name says "profile": the profile being the verified one says
    /// what the boot is allowed to require, not what this boot achieved. What
    /// this boot achieved is [`BootActivation::verified`].
    pub const fn is_verified_profile(self) -> bool {
        matches!(self, BootPolicy::Verified)
    }

    /// Parses a profile token, accepting nothing but the token itself.
    ///
    /// WHY an unrecognised token is `None` and not a default: a request the
    /// kernel cannot name must never be rounded to the profile that happens to
    /// be convenient. The caller reports it and keeps the build default.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let token = core::str::from_utf8(bytes).ok()?;
        let token = token.trim();
        if token.eq_ignore_ascii_case(BOOT_POLICY_VERIFIED_TOKEN) {
            Some(BootPolicy::Verified)
        } else if token.eq_ignore_ascii_case(BOOT_POLICY_UNVERIFIED_DEVELOPMENT_TOKEN) {
            Some(BootPolicy::UnverifiedDevelopment)
        } else {
            None
        }
    }
}

/// The profile this kernel image was built to run under.
///
/// WHY the fallback is the strict profile: the build script refuses to emit an
/// unknown token, so the fallback is unreachable, and if it ever were reached
/// the safe direction is the profile that refuses rather than the one that
/// continues.
pub fn build_default_boot_policy() -> BootPolicy {
    BootPolicy::parse(BUILD_DEFAULT_BOOT_POLICY_TOKEN.as_bytes()).unwrap_or(BootPolicy::Verified)
}

/// The profile this boot runs under, once the boot path has resolved it.
static ACTIVE_BOOT_POLICY: AtomicU8 = AtomicU8::new(0);

const POLICY_CODE_VERIFIED: u8 = 1;
const POLICY_CODE_UNVERIFIED_DEVELOPMENT: u8 = 2;

/// The profile this boot runs under.
///
/// WHY an unset state resolves to the build default rather than to a guess:
/// the boot path resolves the profile before anything reads it, and a caller
/// that asks before the boot path did gets the profile the image was built for.
pub fn boot_policy() -> BootPolicy {
    match ACTIVE_BOOT_POLICY.load(Ordering::Acquire) {
        POLICY_CODE_VERIFIED => BootPolicy::Verified,
        POLICY_CODE_UNVERIFIED_DEVELOPMENT => BootPolicy::UnverifiedDevelopment,
        _ => build_default_boot_policy(),
    }
}

/// Records the profile this boot runs under. Only the boot path calls this.
pub fn set_boot_policy(policy: BootPolicy) {
    let code = match policy {
        BootPolicy::Verified => POLICY_CODE_VERIFIED,
        BootPolicy::UnverifiedDevelopment => POLICY_CODE_UNVERIFIED_DEVELOPMENT,
    };
    ACTIVE_BOOT_POLICY.store(code, Ordering::Release);
}

/// How the profile for one boot was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicySelection {
    /// The profile this boot runs under.
    pub policy: BootPolicy,
    /// The profile the kernel image was built with.
    pub build_default: BootPolicy,
    /// The profile the device asked for, when it asked for one.
    pub requested: Option<BootPolicy>,
    /// Whether a request to leave the verified profile was refused.
    pub refused_downgrade: bool,
}

/// Resolves the profile for this boot.
///
/// WHY the downgrade rule lives here and not in the boot path: "a verified
/// build cannot be talked out of the verified profile" is the whole point of
/// the profile, and a boot path that resolved the profile itself could get
/// that rule wrong with no test failing.
pub fn select_boot_policy(build_default: BootPolicy, requested: Option<BootPolicy>) -> PolicySelection {
    let (policy, refused_downgrade) = match requested {
        None => (build_default, false),
        Some(BootPolicy::Verified) => (BootPolicy::Verified, false),
        Some(BootPolicy::UnverifiedDevelopment) => match build_default {
            // A verified build stays verified: the request comes from the
            // device, and the device is what the profile exists to protect.
            BootPolicy::Verified => (BootPolicy::Verified, true),
            BootPolicy::UnverifiedDevelopment => (BootPolicy::UnverifiedDevelopment, false),
        },
    };
    PolicySelection { policy, build_default, requested, refused_downgrade }
}

/// Where a provisioning tool leaves the boot integrity profile request.
///
/// WHY a device-side file and not another build flag: the profile decides what
/// the boot is allowed to do, and the device is the thing that has to be able
/// to say "no". The request lives in the same `/provision/` namespace as the
/// boot profile token, because it is the same kind of record: a device-side
/// choice read off the data disk before the services start.
pub const BOOT_POLICY_REQUEST_PATH: &str = "/provision/boot.policy";

/// The most request bytes the kernel reads.
const BOOT_POLICY_REQUEST_WINDOW: usize = 64;

const _: () = assert!(
    BOOT_POLICY_REQUEST_PATH.len() <= crate::storage::interfs::MAX_PATH,
    "the boot policy request path must fit one InterFS inode record"
);

/// Reads the profile the device asks for, if it asks for one at all.
///
/// WHY an unreadable request is no request: the ordinary device carries no
/// request file, and a missing file must not cost the device the profile its
/// image was built for.
pub fn load_boot_policy_request_from_device() -> Option<BootPolicy> {
    let mut bytes = [0u8; BOOT_POLICY_REQUEST_WINDOW];
    let Ok(read) = crate::storage::filesystem::load_data_file(BOOT_POLICY_REQUEST_PATH, &mut bytes)
    else {
        return None;
    };
    BootPolicy::parse(&bytes[..read])
}

/// Resolves the profile for this boot from the build default and the device
/// request, records it, and says so on the console.
pub fn resolve_boot_policy() -> PolicySelection {
    let build_default = build_default_boot_policy();
    let requested = load_boot_policy_request_from_device();
    let selection = select_boot_policy(build_default, requested);
    set_boot_policy(selection.policy);
    match selection.requested {
        None => crate::serial_println!(
            "[BOOT] Boot integrity profile: {} (build-time default; no device request at {}).",
            selection.policy.token(),
            BOOT_POLICY_REQUEST_PATH
        ),
        Some(requested) if selection.refused_downgrade => crate::serial_println!(
            "[BOOT] Boot integrity profile: {} (REFUSED the device request for '{}' at {}: a verified build cannot be talked out of the verified profile).",
            selection.policy.token(),
            requested.token(),
            BOOT_POLICY_REQUEST_PATH
        ),
        Some(requested) => crate::serial_println!(
            "[BOOT] Boot integrity profile: {} (device request '{}' read from {}).",
            selection.policy.token(),
            requested.token(),
            BOOT_POLICY_REQUEST_PATH
        ),
    }
    crate::serial_println!(
        "[BOOT] Boot integrity scope: software-trusted prototype boundary -- the trust anchor is the kernel image; this is NOT hardware secure boot (no secure element, no measured-boot register and no ROM trust anchor is bound to this device)."
    );
    selection
}

/// A protected object offered for boot-time activation.
#[derive(Debug, Clone, Copy)]
pub struct ProtectedObject<'a> {
    /// The object's canonical VFS path.
    pub path: &'a str,
    /// The digest the armed manifest attests for this path, if it attests one.
    pub attested_digest: Option<&'a [u8; SHA256_DIGEST_LENGTH]>,
    /// The digest the object's bytes hash to right now, if they could be read.
    pub content_digest: Option<&'a [u8; SHA256_DIGEST_LENGTH]>,
}

/// Why a boot-time activation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityRefusal {
    /// No trust data: the inventory is empty, or an object has no attested digest.
    MissingTrustData,
    /// The manifest is not a list of canonical, unique paths with digests.
    MalformedManifest,
    /// A protected object's bytes do not hash to the digest the manifest attests.
    TamperedProtectedContent,
    /// The object could not be read back, so it could not be authenticated.
    UnreadableProtectedObject,
}

/// What the boot did with the protected system objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootActivation {
    /// The profile the activation ran under.
    pub policy: BootPolicy,
    /// Whether the protected objects are authenticated and may be served.
    pub activated: bool,
    /// Whether this boot may be reported as verified.
    pub verified: bool,
    /// How many protected objects were authenticated.
    pub protected_objects: usize,
    /// The refusal, when there was one.
    pub refusal: Option<IntegrityRefusal>,
}

/// Authenticates the protected system objects and applies the profile's rule.
///
/// WHY the profile decides the shape of the answer and not only its label:
/// under `Verified` a refusal is an `Err`, and the boot path has no way to
/// turn that into a running device -- that is the point of the profile. Under
/// `UnverifiedDevelopment` the same refusal is an `Ok` with `verified = false`
/// and the reason attached, because the device is expected to come up and to
/// say exactly what it did not check.
pub fn activate_protected_objects(
    policy: BootPolicy,
    objects: &[ProtectedObject<'_>],
) -> Result<BootActivation, IntegrityRefusal> {
    match authenticate_protected_objects(objects) {
        Ok(count) => Ok(BootActivation {
            policy,
            activated: true,
            verified: policy.is_verified_profile(),
            protected_objects: count,
            refusal: None,
        }),
        Err(refusal) if policy.refuses_activation_on_failure() => Err(refusal),
        Err(refusal) => Ok(BootActivation {
            policy,
            activated: false,
            verified: false,
            protected_objects: 0,
            refusal: Some(refusal),
        }),
    }
}

/// Authenticates the protected objects against the manifest that attests them.
///
/// WHY the checks are in one place: the profile decides what a failure means,
/// and it can only do that if both profiles run exactly the same checks.
fn authenticate_protected_objects(objects: &[ProtectedObject<'_>]) -> Result<usize, IntegrityRefusal> {
    if objects.is_empty() {
        // No inventory means no trust data at all: there is nothing to
        // authenticate against, and a chain with no anchor is not a chain.
        return Err(IntegrityRefusal::MissingTrustData);
    }
    let mut seen: Vec<&str> = Vec::with_capacity(objects.len());
    for object in objects {
        if object.path.is_empty() || !object.path.starts_with('/') || object.path.contains('\\') {
            return Err(IntegrityRefusal::MalformedManifest);
        }
        if seen.iter().any(|seen_path| *seen_path == object.path) {
            return Err(IntegrityRefusal::MalformedManifest);
        }
        seen.push(object.path);
        let Some(attested) = object.attested_digest else {
            return Err(IntegrityRefusal::MissingTrustData);
        };
        let Some(content) = object.content_digest else {
            return Err(IntegrityRefusal::UnreadableProtectedObject);
        };
        if attested != content {
            return Err(IntegrityRefusal::TamperedProtectedContent);
        }
    }
    Ok(objects.len())
}

/// The provisioned system-image Merkle root, or `None` until the boot path
/// installs it from authorized boot metadata. Protected reads fail closed while
/// it is `None`; the platform reports integrity as NOT IMPLEMENTED rather than
/// fabricating a root.
///
/// WHY the name says "inventory": the value is the digest of the RUNTIME
/// read-only system inventory (the VFS's `ReadOnlySystem` objects). It is not
/// the build-time embedded-image manifest digest, which covers different data
/// with a different encoding, and the two are never compared.
static PROVISIONED_INVENTORY_DIGEST: Mutex<Option<[u8; SHA256_DIGEST_LENGTH]>> = Mutex::new(None);

/// The authenticated metadata manifest installed with the inventory digest:
/// the set of `(name, sha256)` pairs the digest commits to. A `ReadOnlySystem`
/// inode is trusted only when its own digest appears here under the
/// provisioned digest, so an attacker who can write the inode tree cannot
/// substitute a digest the inventory never attested.
static PROVISIONED_INVENTORY_MANIFEST: Mutex<Option<Vec<(alloc::string::String, [u8; SHA256_DIGEST_LENGTH])>>> =
    Mutex::new(None);

/// Boot metadata that authorizes a system-image root. A root is trustworthy only
/// when it is the digest of a manifest the firmware/build measured, not the first
/// nonzero value a caller passes.
///
/// WHY the field says "inventory": it carries the digest of the RUNTIME
/// read-only system inventory, not the build-time embedded-image manifest
/// digest. Those are different values over different data (see
/// [`system_inventory_manifest_digest`] and `kernel/build.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootAttestation {
    pub inventory_manifest_digest: [u8; SHA256_DIGEST_LENGTH],
}

/// Computes the digest OF the runtime read-only system inventory.
///
/// WHY this encoding and not the build script's: the build-time
/// `EMBEDDED_IMAGE_MANIFEST_SHA256` hashes `name || digest` over the embedded
/// Ring-3 image names, while this hashes `u64 name_len || name || digest` over
/// the canonical VFS paths of the read-only system objects. The length prefix
/// makes an unambiguous encoding, and because the covered data and the
/// encoding both differ, the two digests are different values and are never
/// compared with each other.
fn system_inventory_manifest_digest(entries: &[(&str, [u8; SHA256_DIGEST_LENGTH])]) -> [u8; SHA256_DIGEST_LENGTH] {
    let mut buf = Vec::new();
    for (name, digest) in entries {
        buf.extend_from_slice(&(name.len() as u64).to_le_bytes());
        buf.extend_from_slice(name.as_bytes());
        buf.extend_from_slice(digest);
    }
    Sha256::digest(&buf)
}

/// Installs the trusted system-image root and its authenticated metadata manifest
/// from authorized boot metadata. The root is accepted only when it equals the
/// boot-attested inventory digest AND the supplied entries recompute to that
/// digest; a caller cannot install an arbitrary nonzero root. Duplicate canonical
/// paths are refused so one entry can never shadow another.
pub fn provision_root_from_boot(
    attestation: &BootAttestation,
    root: &[u8; SHA256_DIGEST_LENGTH],
    entries: &[(&str, [u8; SHA256_DIGEST_LENGTH])],
) -> Result<(), StorageError> {
    if root == &[0u8; SHA256_DIGEST_LENGTH] {
        return Err(StorageError::UnauthenticatedRoot);
    }
    if root != &attestation.inventory_manifest_digest {
        return Err(StorageError::UnauthenticatedRoot);
    }
    if &system_inventory_manifest_digest(entries) != root {
        return Err(StorageError::UnauthenticatedRoot);
    }
    // Reject duplicate canonical paths and empty/invalid names: an ambiguous
    // manifest could authorize one object's digest for another.
    let mut seen: Vec<&str> = Vec::with_capacity(entries.len());
    for (name, _) in entries {
        if name.is_empty() || name.contains('\\') || !name.starts_with('/') {
            return Err(StorageError::UnauthenticatedRoot);
        }
        if seen.iter().any(|n| *n == *name) {
            return Err(StorageError::UnauthenticatedRoot);
        }
        seen.push(name);
    }
    let owned: Vec<(alloc::string::String, [u8; SHA256_DIGEST_LENGTH])> =
        entries.iter().map(|(n, d)| (alloc::string::String::from(*n), *d)).collect();
    let mut slot = PROVISIONED_INVENTORY_DIGEST.lock();
    match *slot {
        None => {
            *slot = Some(*root);
            *PROVISIONED_INVENTORY_MANIFEST.lock() = Some(owned);
            Ok(())
        }
        Some(existing) if &existing == root => Ok(()),
        Some(_) => Err(StorageError::UnauthenticatedRoot),
    }
}

/// The digest OF the runtime read-only system inventory, or `None`.
///
/// WHY this is not the build-time embedded-image manifest digest: that digest
/// covers the embedded Ring-3 image names and their file digests, while this
/// one covers the canonical VFS paths of the read-only system objects with a
/// length-prefixed encoding. Different data, different encoding, different
/// value -- and nothing in the kernel compares the two.
pub fn provisioned_root() -> Option<[u8; SHA256_DIGEST_LENGTH]> {
    *PROVISIONED_INVENTORY_DIGEST.lock()
}

/// The armed inventory: the `(path, digest)` pairs the inventory digest commits to.
///
/// WHY a copy and not a borrow: the manifest lives behind a lock, and a caller
/// that held it while reading the objects it names would hold the storage lock
/// across filesystem I/O. The boot path takes a copy and drops the lock.
pub fn provisioned_inventory() -> Vec<(alloc::string::String, [u8; SHA256_DIGEST_LENGTH])> {
    PROVISIONED_INVENTORY_MANIFEST
        .lock()
        .as_ref()
        .map(|entries| entries.clone())
        .unwrap_or_default()
}

/// Provisions the protected-storage trust anchor from the kernel's own
/// authenticated `ReadOnlySystem` inventory at boot.
///
/// WHY this is an authorized source and not first-writer enrollment: the entries
/// are the VFS's read-only system inodes, which the kernel itself built from its
/// own embedded constants (the embedded ELFs are already verified against the
/// build-time `EXPECTED_IMAGE_DIGESTS` at boot). The anchor is recomputed
/// deterministically from that inventory each boot, so an attacker who can only
/// write the data partition cannot change it; the trust anchor is the kernel
/// image (software-trusted, NOT hardware secure boot).
///
/// WHY the anchor is NOT the build-time image manifest digest: that digest
/// covers the embedded Ring-3 images with the build script's own encoding
/// (`name || digest`), while this one covers the VFS's read-only objects with a
/// length-prefixed encoding (`path_len || path || digest`). They are different
/// values over different data, they are named apart, and no code compares them.
/// An empty inventory leaves the chain inert (NOT IMPLEMENTED).
pub fn provision_from_inventory(entries: &[(&str, [u8; SHA256_DIGEST_LENGTH])]) -> Result<(), StorageError> {
    if entries.is_empty() {
        return Err(StorageError::UnauthenticatedRoot);
    }
    let root = system_inventory_manifest_digest(entries);
    let attestation = BootAttestation { inventory_manifest_digest: root };
    provision_root_from_boot(&attestation, &root, entries)
}

/// Re-verifies protected metadata: the inventory must be provisioned and the
/// object's **full canonical path** and `recorded_digest` must be attested by
/// the armed manifest. Basename matching is never used for authorization, so
/// two files with the same basename in different directories are distinct
/// objects.
pub fn verify_protected_metadata(
    path: &str,
    recorded_digest: &[u8; SHA256_DIGEST_LENGTH],
) -> Result<(), StorageError> {
    provisioned_root().ok_or(StorageError::UnauthenticatedRoot)?;
    let manifest = PROVISIONED_INVENTORY_MANIFEST.lock();
    let attested = manifest
        .as_ref()
        .map(|entries| entries.iter().any(|(n, d)| n.as_str() == path && d == recorded_digest))
        .unwrap_or(false);
    if attested { Ok(()) } else { Err(StorageError::IntegrityViolation) }
}

/// Counts the protected objects the boot refused to serve.
static PROTECTED_READ_REFUSALS: AtomicUsize = AtomicUsize::new(0);

/// Whether the first protected-read refusal has already been reported.
static PROTECTED_READ_REFUSAL_REPORTED: AtomicU8 = AtomicU8::new(0);

/// Reports a refused protected read.
///
/// WHY only the first refusal is printed: the console is a shared, lossy
/// channel, and an object that fails every time it is read would otherwise
/// turn one tampered file into a console flood. The counter keeps the rest,
/// so the number of refusals is still visible to whoever asks.
fn report_protected_read_refusal(path: &str, reason: StorageError) {
    PROTECTED_READ_REFUSALS.fetch_add(1, Ordering::Relaxed);
    if PROTECTED_READ_REFUSAL_REPORTED.swap(1, Ordering::AcqRel) == 0 {
        crate::serial_println!(
            "[STORAGE] PROTECTED OBJECT REFUSED under profile '{}': path={} reason={:?} (further refusals are counted, not printed).",
            boot_policy().token(),
            path,
            reason
        );
    }
}

/// How many protected objects the boot refused to serve.
pub fn protected_read_refusal_count() -> usize {
    PROTECTED_READ_REFUSALS.load(Ordering::Relaxed)
}

/// Re-verifies a protected read against the provisioned inventory digest and
/// its authenticated metadata manifest. Fails closed with `UnauthenticatedRoot`
/// when no inventory is provisioned, `IntegrityViolation` when the inode's
/// digest is not attested by the manifest, and `IntegrityViolation` when the
/// data does not match the recorded digest. Every refusal is reported, so a
/// tampered protected object cannot be served silently.
pub fn verify_protected_read(
    path: &str,
    recorded_digest: &[u8; SHA256_DIGEST_LENGTH],
    data: &[u8],
) -> Result<(), StorageError> {
    if let Err(reason) = verify_protected_metadata(path, recorded_digest) {
        report_protected_read_refusal(path, reason);
        return Err(reason);
    }
    if let Err(reason) = MerkleVerifier::verify_block(data, recorded_digest) {
        report_protected_read_refusal(path, reason);
        return Err(reason);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn reset_root_for_test() {
    *PROVISIONED_INVENTORY_DIGEST.lock() = None;
    *PROVISIONED_INVENTORY_MANIFEST.lock() = None;
    PROTECTED_READ_REFUSALS.store(0, Ordering::Relaxed);
    PROTECTED_READ_REFUSAL_REPORTED.store(0, Ordering::Relaxed);
}

/// An authenticated block reader connecting physical or virtual block storage
/// to cryptographic Merkle proof verification, ensuring block authentication before use.
pub struct AuthenticatedBlockReader<D> {
    device: D,
    verifier: MerkleVerifier,
    total_blocks: usize,
}

impl<D> AuthenticatedBlockReader<D> {
    /// Creates a new authenticated block reader anchored by a trusted Merkle root.
    pub const fn new(device: D, verifier: MerkleVerifier, total_blocks: usize) -> Self {
        Self {
            device,
            verifier,
            total_blocks,
        }
    }

    /// Access the underlying block device.
    pub fn device(&self) -> &D {
        &self.device
    }

    /// Access the underlying block device mutably.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    /// Access the verifier.
    pub fn verifier(&self) -> &MerkleVerifier {
        &self.verifier
    }

    /// Total verified block count.
    pub fn total_blocks(&self) -> usize {
        self.total_blocks
    }
}

impl<D: BlockDevice> AuthenticatedBlockReader<D> {
    /// Reads a block from the underlying block device and cryptographically authenticates it
    /// against the anchored Merkle root before returning the verified data to callers.
    pub fn read_authenticated_block(
        &mut self,
        block_index: usize,
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
        out: &mut [u8; VERIFIED_BLOCK_SIZE],
    ) -> Result<(), StorageError> {
        if block_index >= self.total_blocks {
            return Err(StorageError::BlockOutOfRange);
        }
        if proof.len() > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }

        self.device
            .read_sector(block_index as u32, out)
            .map_err(|_| StorageError::DeviceError)?;

        self.verifier.verify_branch_proof(out, block_index, proof)
    }
}

/// Layout and cryptographic proof geometry for a protected system image partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemImageGeometry {
    pub start_lba: u32,
    pub block_count: usize,
    pub block_size: usize,
    pub tree_depth: usize,
}

impl SystemImageGeometry {
    /// Computes the exact tree depth required for `block_count` leaves.
    pub const fn compute_depth(block_count: usize) -> usize {
        if block_count <= 1 {
            return 0;
        }
        let mut depth = 0;
        let mut count = block_count - 1;
        while count > 0 {
            count >>= 1;
            depth += 1;
        }
        depth
    }

    /// Creates and validates system image partition geometry.
    pub fn new(start_lba: u32, block_count: usize) -> Result<Self, StorageError> {
        if block_count == 0 {
            return Err(StorageError::BlockOutOfRange);
        }
        let depth = Self::compute_depth(block_count);
        if depth > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }
        Ok(Self {
            start_lba,
            block_count,
            block_size: VERIFIED_BLOCK_SIZE,
            tree_depth: depth,
        })
    }
}

/// A protected system image reader that enforces fail-closed cryptographic Merkle verification
/// on every block read from the system partition.
///
/// Protected reads cannot bypass verification: any attempt to read from this partition
/// requires a cryptographically valid branch proof leading to the authenticated Merkle root.
pub struct ProtectedSystemImage<D> {
    device: D,
    geometry: SystemImageGeometry,
    verifier: MerkleVerifier,
}

impl<D> ProtectedSystemImage<D> {
    /// Anchors the protected system image with an authenticated root and partition geometry.
    pub fn new(
        device: D,
        geometry: SystemImageGeometry,
        root_digest: [u8; SHA256_DIGEST_LENGTH],
    ) -> Result<Self, StorageError> {
        if root_digest == [0u8; SHA256_DIGEST_LENGTH] {
            return Err(StorageError::UnauthenticatedRoot);
        }
        Ok(Self {
            device,
            geometry,
            verifier: MerkleVerifier::new(root_digest),
        })
    }

    pub fn geometry(&self) -> &SystemImageGeometry {
        &self.geometry
    }

    pub fn root_digest(&self) -> &[u8; SHA256_DIGEST_LENGTH] {
        self.verifier.root_digest()
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }
}

impl<D: BlockDevice> ProtectedSystemImage<D> {
    /// Reads and cryptographically authenticates a block from the protected system partition.
    ///
    /// Fails closed: if boundary check, proof geometry, device I/O, or SHA-256 branch proof fails,
    /// `out` is zeroized and the error is returned immediately. Unauthenticated data is never leaked.
    pub fn read_protected_block(
        &mut self,
        relative_lba: usize,
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
        out: &mut [u8; VERIFIED_BLOCK_SIZE],
    ) -> Result<(), StorageError> {
        // 1. Boundary check: ensure read is strictly within partition geometry
        if relative_lba >= self.geometry.block_count {
            out.fill(0);
            return Err(StorageError::BlockOutOfRange);
        }

        // 2. Exact proof geometry check
        if proof.len() != self.geometry.tree_depth {
            out.fill(0);
            return Err(StorageError::GeometryMismatch);
        }

        // 3. Compute absolute LBA on hardware storage device
        let physical_lba = self.geometry.start_lba
            .checked_add(relative_lba as u32)
            .ok_or_else(|| {
                out.fill(0);
                StorageError::BlockOutOfRange
            })?;

        // 4. Read raw sector from hardware storage device into private buffer
        let mut temp_buf = [0u8; VERIFIED_BLOCK_SIZE];
        if self.device.read_sector(physical_lba, &mut temp_buf).is_err() {
            out.fill(0);
            return Err(StorageError::DeviceError);
        }

        // 5. Cryptographic Merkle branch verification against authenticated root
        match self.verifier.verify_branch_proof(&temp_buf, relative_lba, proof) {
            Ok(()) => {
                out.copy_from_slice(&temp_buf);
                Ok(())
            }
            Err(e) => {
                out.fill(0);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block::BlockDeviceError;

    struct MockBlockDevice {
        start_lba: u32,
        blocks: [[u8; VERIFIED_BLOCK_SIZE]; 4],
        fail_reads: bool,
    }

    impl BlockDevice for MockBlockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if self.fail_reads {
                return Err(BlockDeviceError::ReadError);
            }
            if lba >= self.start_lba {
                let idx = (lba - self.start_lba) as usize;
                if idx < self.blocks.len() {
                    buf.copy_from_slice(&self.blocks[idx]);
                    return Ok(());
                }
            }
            Err(BlockDeviceError::LbaOutOfRange)
        }

        fn write_sector(&mut self, _lba: u32, _buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            Ok(())
        }

        fn sector_count(&self) -> u32 {
            self.start_lba + self.blocks.len() as u32
        }

        fn model(&self) -> &str {
            "MockVerifiedStorage"
        }
    }

    #[test]
    fn merkle_branch_proof_valid_and_tampered() {
        let leaf0 = b"Partition block 0 content";
        let leaf1 = b"Partition block 1 content";
        let leaf2 = b"Partition block 2 content";
        let leaf3 = b"Partition block 3 content";

        let h0 = Sha256::digest(leaf0);
        let h1 = Sha256::digest(leaf1);
        let h2 = Sha256::digest(leaf2);
        let h3 = Sha256::digest(leaf3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        let verifier = MerkleVerifier::new(root);

        // Valid proof for leaf 0: audit path = [h1, parent1]
        assert_eq!(verifier.verify_branch_proof(leaf0, 0, &[h1, parent1]), Ok(()));

        // Valid proof for leaf 2: audit path = [h3, parent0]
        assert_eq!(verifier.verify_branch_proof(leaf2, 2, &[h3, parent0]), Ok(()));

        // Tampered leaf data must be rejected
        let tampered_leaf = b"Partition block 0 TAMPERED";
        assert_eq!(
            verifier.verify_branch_proof(tampered_leaf, 0, &[h1, parent1]),
            Err(StorageError::IntegrityViolation)
        );

        // Tampered sibling in audit path must be rejected
        let mut tampered_h1 = h1;
        tampered_h1[0] ^= 0xFF;
        assert_eq!(
            verifier.verify_branch_proof(leaf0, 0, &[tampered_h1, parent1]),
            Err(StorageError::IntegrityViolation)
        );
    }

    #[test]
    fn authenticated_block_reader_full_lifecycle() {
        let mut b0 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b1 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b2 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b3 = [0u8; VERIFIED_BLOCK_SIZE];

        b0[0..12].copy_from_slice(b"RootFsBlock0");
        b1[0..12].copy_from_slice(b"RootFsBlock1");
        b2[0..12].copy_from_slice(b"RootFsBlock2");
        b3[0..12].copy_from_slice(b"RootFsBlock3");

        let h0 = Sha256::digest(&b0);
        let h1 = Sha256::digest(&b1);
        let h2 = Sha256::digest(&b2);
        let h3 = Sha256::digest(&b3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        let verifier = MerkleVerifier::new(root);
        let dev = MockBlockDevice {
            start_lba: 0,
            blocks: [b0, b1, b2, b3],
            fail_reads: false,
        };

        let mut reader = AuthenticatedBlockReader::new(dev, verifier, 4);

        // 1. Valid read of block 0
        let mut out = [0u8; VERIFIED_BLOCK_SIZE];
        assert_eq!(reader.read_authenticated_block(0, &[h1, parent1], &mut out), Ok(()));
        assert_eq!(&out[0..12], b"RootFsBlock0");

        // 2. Valid read of block 2
        assert_eq!(reader.read_authenticated_block(2, &[h3, parent0], &mut out), Ok(()));
        assert_eq!(&out[0..12], b"RootFsBlock2");

        // 3. Out of bounds block index
        assert_eq!(
            reader.read_authenticated_block(4, &[h1, parent1], &mut out),
            Err(StorageError::BlockOutOfRange)
        );

        // 4. Proof depth overflow
        let deep_proof = [[0u8; SHA256_DIGEST_LENGTH]; MAX_MERKLE_PROOF_DEPTH + 1];
        assert_eq!(
            reader.read_authenticated_block(0, &deep_proof, &mut out),
            Err(StorageError::BlockOutOfRange)
        );

        // 5. Tampered block on storage device
        reader.device_mut().blocks[0][0] ^= 0x55;
        assert_eq!(
            reader.read_authenticated_block(0, &[h1, parent1], &mut out),
            Err(StorageError::IntegrityViolation)
        );

        // 6. Device I/O failure
        reader.device_mut().fail_reads = true;
        assert_eq!(
            reader.read_authenticated_block(2, &[h3, parent0], &mut out),
            Err(StorageError::DeviceError)
        );
    }

    #[test]
    fn protected_system_image_fail_closed_and_geometry() {
        let mut b0 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b1 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b2 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b3 = [0u8; VERIFIED_BLOCK_SIZE];

        b0[0..16].copy_from_slice(b"SystemPartition0");
        b1[0..16].copy_from_slice(b"SystemPartition1");
        b2[0..16].copy_from_slice(b"SystemPartition2");
        b3[0..16].copy_from_slice(b"SystemPartition3");

        let h0 = Sha256::digest(&b0);
        let h1 = Sha256::digest(&b1);
        let h2 = Sha256::digest(&b2);
        let h3 = Sha256::digest(&b3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        // Partition offset LBA 2048, 4 blocks
        let geometry = SystemImageGeometry::new(2048, 4).expect("valid geometry");
        assert_eq!(geometry.tree_depth, 2);
        assert_eq!(geometry.start_lba, 2048);
        assert_eq!(geometry.block_count, 4);

        // Reject all-zero unauthenticated root
        let zero_root = [0u8; SHA256_DIGEST_LENGTH];
        let dev0 = MockBlockDevice { start_lba: 2048, blocks: [b0, b1, b2, b3], fail_reads: false };
        assert_eq!(
            ProtectedSystemImage::new(dev0, geometry, zero_root).map(|_| ()),
            Err(StorageError::UnauthenticatedRoot)
        );

        let dev = MockBlockDevice { start_lba: 2048, blocks: [b0, b1, b2, b3], fail_reads: false };
        let mut sys_img = ProtectedSystemImage::new(dev, geometry, root).expect("valid image");

        // 1. Valid read of block 0 with exact proof geometry (depth = 2)
        let mut out = [0u8; VERIFIED_BLOCK_SIZE];
        assert_eq!(sys_img.read_protected_block(0, &[h1, parent1], &mut out), Ok(()));
        assert_eq!(&out[0..16], b"SystemPartition0");

        // 2. Valid read of block 3
        assert_eq!(sys_img.read_protected_block(3, &[h2, parent0], &mut out), Ok(()));
        assert_eq!(&out[0..16], b"SystemPartition3");

        // 3. Proof geometry mismatch (depth 1 instead of 2) -> fail-closed, output zeroed
        out.fill(0xAA);
        assert_eq!(
            sys_img.read_protected_block(0, &[h1], &mut out),
            Err(StorageError::GeometryMismatch)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 4. Boundary violation (relative_lba >= block_count) -> fail-closed, output zeroed
        out.fill(0xAA);
        assert_eq!(
            sys_img.read_protected_block(4, &[h1, parent1], &mut out),
            Err(StorageError::BlockOutOfRange)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 5. Tampered media block on disk -> fail-closed, output zeroed
        out.fill(0xAA);
        sys_img.device_mut().blocks[0][0] ^= 0x55;
        assert_eq!(
            sys_img.read_protected_block(0, &[h1, parent1], &mut out),
            Err(StorageError::IntegrityViolation)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 6. Device I/O failure -> fail-closed, output zeroed
        out.fill(0xAA);
        sys_img.device_mut().fail_reads = true;
        assert_eq!(
            sys_img.read_protected_block(3, &[h2, parent0], &mut out),
            Err(StorageError::DeviceError)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);
    }

    #[test]
    fn protected_read_requires_authenticated_metadata() {
        reset_root_for_test();
        let data = b"protected system image block";
        let digest = Sha256::digest(data);
        let entries = [("/system/bin/init", digest)];
        let root = system_inventory_manifest_digest(&entries);
        let att = BootAttestation { inventory_manifest_digest: root };

        // No root: protected read fails closed (inert NOT IMPLEMENTED).
        assert_eq!(verify_protected_read("/system/bin/init", &digest, data), Err(StorageError::UnauthenticatedRoot));

        // A root that is not the boot-attested manifest digest is refused.
        let bogus = Sha256::digest(b"attacker-chosen root");
        assert_eq!(provision_root_from_boot(&att, &bogus, &entries), Err(StorageError::UnauthenticatedRoot));
        // Entries that do not recompute to the attested root are refused.
        let other = [("/system/bin/init", Sha256::digest(b"other"))];
        assert_eq!(provision_root_from_boot(&att, &root, &other), Err(StorageError::UnauthenticatedRoot));

        // Authorized provisioning arms the chain.
        assert_eq!(provision_root_from_boot(&att, &root, &entries), Ok(()));
        assert_eq!(verify_protected_read("/system/bin/init", &digest, data), Ok(()));
        // A path the manifest does not attest fails closed (full-path binding).
        assert_eq!(verify_protected_read("/system/etc/policy.json", &digest, data), Err(StorageError::IntegrityViolation));
        // A digest the manifest does not attest fails closed.
        let rogue = Sha256::digest(b"rogue");
        assert_eq!(verify_protected_read("/system/bin/init", &rogue, b"rogue"), Err(StorageError::IntegrityViolation));
        // Tampered data under an attested digest fails closed.
        assert_eq!(verify_protected_read("/system/bin/init", &digest, b"protected system image blocK"), Err(StorageError::IntegrityViolation));

        reset_root_for_test();
        assert!(provisioned_root().is_none());
    }

    #[test]
    fn same_basename_in_different_dirs_is_not_aliased() {
        reset_root_for_test();
        let a = b"init-A";
        let b = b"init-B";
        let da = Sha256::digest(a);
        let db = Sha256::digest(b);
        // Two objects named "init" in different directories, both attested.
        let entries = [("/system/bin/init", da), ("/other/init", db)];
        let root = system_inventory_manifest_digest(&entries);
        let att = BootAttestation { inventory_manifest_digest: root };
        assert_eq!(provision_root_from_boot(&att, &root, &entries), Ok(()));
        // Each full path authorizes only its own digest.
        assert_eq!(verify_protected_read("/system/bin/init", &da, a), Ok(()));
        assert_eq!(verify_protected_read("/other/init", &db, b), Ok(()));
        assert_eq!(verify_protected_read("/system/bin/init", &db, b), Err(StorageError::IntegrityViolation));
        // A duplicate canonical path is refused.
        reset_root_for_test();
        let dup = [("/system/bin/init", da), ("/system/bin/init", db)];
        let root2 = system_inventory_manifest_digest(&dup);
        let att2 = BootAttestation { inventory_manifest_digest: root2 };
        assert_eq!(provision_root_from_boot(&att2, &root2, &dup), Err(StorageError::UnauthenticatedRoot));
        reset_root_for_test();
    }

    // -----------------------------------------------------------------------
    // Boot integrity profile: the negative cases.
    //
    // WHY these are unit tests and not a QEMU run: the profile's whole job is
    // to decide what a failed authentication means, and that decision is made
    // in this module. A guest run would show the consequence, but only these
    // cases can show that EVERY failure is refused, and that the development
    // profile reports the same failure instead of swallowing it.
    // -----------------------------------------------------------------------

    #[test]
    fn the_verified_profile_refuses_a_malformed_manifest() {
        reset_root_for_test();
        let digest = Sha256::digest(b"protected system image block");
        // A path that is not canonical: the manifest would authorize an object
        // the VFS cannot name.
        let not_canonical = ProtectedObject {
            path: "system/bin/init",
            attested_digest: Some(&digest),
            content_digest: Some(&digest),
        };
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[not_canonical]),
            Err(IntegrityRefusal::MalformedManifest)
        );

        // The same path twice: one entry could shadow the other.
        let first = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&digest),
            content_digest: Some(&digest),
        };
        let second = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&digest),
            content_digest: Some(&digest),
        };
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[first, second]),
            Err(IntegrityRefusal::MalformedManifest)
        );
    }

    #[test]
    fn the_verified_profile_refuses_a_tampered_protected_object() {
        reset_root_for_test();
        let attested = Sha256::digest(b"protected system image block");
        let tampered = Sha256::digest(b"protected system image blocK");
        let object = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&attested),
            content_digest: Some(&tampered),
        };
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[object]),
            Err(IntegrityRefusal::TamperedProtectedContent)
        );
    }

    #[test]
    fn the_verified_profile_refuses_missing_trust_data() {
        reset_root_for_test();
        // No inventory at all: there is nothing to authenticate against.
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[]),
            Err(IntegrityRefusal::MissingTrustData)
        );

        let digest = Sha256::digest(b"protected system image block");
        // An object the manifest does not attest: its bytes have no anchor.
        let unattested = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: None,
            content_digest: Some(&digest),
        };
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[unattested]),
            Err(IntegrityRefusal::MissingTrustData)
        );

        // An object that could not be read back could not be authenticated.
        let unreadable = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&digest),
            content_digest: None,
        };
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[unreadable]),
            Err(IntegrityRefusal::UnreadableProtectedObject)
        );
    }

    #[test]
    fn the_unverified_development_profile_boots_and_reports_the_same_failure() {
        reset_root_for_test();
        // WHY this runs the same input as the refusal cases: the development
        // profile must not authenticate anything the verified profile would
        // have refused. It comes up, it says what failed, and it never says
        // "verified".
        let attested = Sha256::digest(b"protected system image block");
        let tampered = Sha256::digest(b"protected system image blocK");
        let object = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&attested),
            content_digest: Some(&tampered),
        };

        let activation = activate_protected_objects(BootPolicy::UnverifiedDevelopment, &[object])
            .expect("the development profile comes up instead of refusing");
        assert!(!activation.activated, "a refused object is not activated");
        assert!(!activation.verified, "a development boot is never verified");
        assert_eq!(activation.refusal, Some(IntegrityRefusal::TamperedProtectedContent));
        assert_eq!(activation.policy, BootPolicy::UnverifiedDevelopment);

        // The same failure under the verified profile is an error, not a
        // record: the boot path has no way to turn it into a running device.
        assert_eq!(
            activate_protected_objects(BootPolicy::Verified, &[object]),
            Err(IntegrityRefusal::TamperedProtectedContent)
        );

        // A development boot that authenticates everything is still not
        // verified, because the profile is what the boot promised.
        let clean = ProtectedObject {
            path: "/system/bin/init",
            attested_digest: Some(&attested),
            content_digest: Some(&attested),
        };
        let activation = activate_protected_objects(BootPolicy::UnverifiedDevelopment, &[clean])
            .expect("a development boot with a clean inventory comes up");
        assert!(activation.activated);
        assert!(!activation.verified, "a development boot must never be reported as verified");
        assert_eq!(activation.protected_objects, 1);
    }

    #[test]
    fn a_verified_build_cannot_be_talked_out_of_the_verified_profile() {
        // No request: the build default stands.
        let selection = select_boot_policy(BootPolicy::Verified, None);
        assert_eq!(selection.policy, BootPolicy::Verified);
        assert!(!selection.refused_downgrade);

        // A verified build asked to run unverified refuses and says so.
        let refused = select_boot_policy(BootPolicy::Verified, Some(BootPolicy::UnverifiedDevelopment));
        assert_eq!(refused.policy, BootPolicy::Verified);
        assert!(refused.refused_downgrade);
        assert_eq!(refused.requested, Some(BootPolicy::UnverifiedDevelopment));

        // A development build may be asked for the stricter profile.
        let upgraded = select_boot_policy(BootPolicy::UnverifiedDevelopment, Some(BootPolicy::Verified));
        assert_eq!(upgraded.policy, BootPolicy::Verified);
        assert!(!upgraded.refused_downgrade);

        // A development build asked for the development profile stays there.
        let same = select_boot_policy(
            BootPolicy::UnverifiedDevelopment,
            Some(BootPolicy::UnverifiedDevelopment),
        );
        assert_eq!(same.policy, BootPolicy::UnverifiedDevelopment);
        assert!(!same.refused_downgrade);
    }

    #[test]
    fn a_boot_policy_token_is_accepted_only_when_it_names_a_profile() {
        assert_eq!(BootPolicy::parse(b"verified"), Some(BootPolicy::Verified));
        assert_eq!(BootPolicy::parse(b"  VERIFIED \n"), Some(BootPolicy::Verified));
        assert_eq!(
            BootPolicy::parse(b"unverified-development"),
            Some(BootPolicy::UnverifiedDevelopment)
        );
        assert_eq!(BootPolicy::parse(b"unverified"), None);
        assert_eq!(BootPolicy::parse(b""), None);
        assert_eq!(BootPolicy::parse(&[0xff, 0xfe]), None);
    }

    #[test]
    fn the_build_default_names_one_of_the_two_profiles() {
        // WHY this is asserted here and not only in the build script: the
        // kernel reads the token the build script wrote, and an image whose
        // token names nothing would resolve to a profile nobody chose.
        assert!(
            BUILD_DEFAULT_BOOT_POLICY_TOKEN == BOOT_POLICY_VERIFIED_TOKEN
                || BUILD_DEFAULT_BOOT_POLICY_TOKEN == BOOT_POLICY_UNVERIFIED_DEVELOPMENT_TOKEN,
            "the build-time profile token must name a profile"
        );
        assert_eq!(build_default_boot_policy().token(), BUILD_DEFAULT_BOOT_POLICY_TOKEN);
    }

    #[test]
    fn the_runtime_inventory_digest_is_not_the_build_time_image_manifest_digest() {
        // WHY this test exists: the kernel once carried a build-time manifest
        // digest and a runtime manifest digest under similar names, and a
        // reader could take the two for the same value. They cover different
        // data with different encodings, so they are different values, and
        // nothing may compare them.
        let entries = [("/system/bin/init", Sha256::digest(b"init-bytes"))];
        let runtime = system_inventory_manifest_digest(&entries);

        // The build script's own encoding over the very same pair: the name
        // bytes, then the 32 raw digest bytes, with no length prefix.
        let mut build_hasher = Sha256::new();
        for (name, digest) in entries {
            build_hasher.update(name.as_bytes());
            build_hasher.update(&digest);
        }
        let build_time: [u8; 32] = build_hasher.finalize();
        assert_ne!(
            runtime, build_time,
            "the runtime inventory digest and the build-time image manifest digest must not be one value"
        );

        // The runtime digest is over the inventory entries, in order.
        let reordered = [
            ("/system/bin/other", Sha256::digest(b"other")),
            ("/system/bin/init", Sha256::digest(b"init-bytes")),
        ];
        assert_ne!(runtime, system_inventory_manifest_digest(&reordered));
    }

    #[test]
    fn a_refused_protected_read_is_counted_and_reported() {
        reset_root_for_test();
        let data = b"protected system image block";
        let digest = Sha256::digest(data);
        let entries = [("/system/bin/init", digest)];
        let root = system_inventory_manifest_digest(&entries);
        let att = BootAttestation { inventory_manifest_digest: root };
        assert_eq!(provision_root_from_boot(&att, &root, &entries), Ok(()));
        assert_eq!(protected_read_refusal_count(), 0);

        // Tampered bytes under an attested digest: the read is refused and the
        // refusal is visible, not silent.
        assert_eq!(
            verify_protected_read("/system/bin/init", &digest, b"protected system image blocK"),
            Err(StorageError::IntegrityViolation)
        );
        assert_eq!(protected_read_refusal_count(), 1);
        reset_root_for_test();
        assert_eq!(protected_read_refusal_count(), 0);
    }
}
