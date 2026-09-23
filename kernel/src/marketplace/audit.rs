//! Zero-Trust Pre-Flight Capability Auditor for Marketplace Automations
//!
//! Enforces Principle #2: Security is enforced by the kernel below user workloads.
//! Prevents rogue or overprivileged third-party packages from compromising hardware.

use crate::package::{AmcpPackage, CapabilityRisk};
use super::catalog::MarketplaceItem;

/// Pre-flight security audit verdict for package installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditVerdict {
    /// Safe capabilities only; approved for automatic installation.
    Approved = 0,
    /// Requests network or filesystem write operations; requires explicit user consent.
    RequiresUserConsent = 1,
    /// Requests raw hardware I/O, raw disk, or kernel killing; strictly blocked.
    BlockedForbidden = 2,
    /// Cryptographic developer signature mismatch; potential supply-chain tampering.
    SignatureInvalid = 3,
    /// Package or developer signing key has been revoked on sovereign CRL.
    PackageRevoked = 4,
}

/// Evaluates an automation package against kernel security policy rules.
pub fn audit_item(item: &MarketplaceItem) -> AuditVerdict {
    // 0. Check decentralized Certificate Revocation List (CRL)
    if super::revocation::REVOCATION_REGISTRY.lock().is_package_revoked(&item.package_id)
        || super::revocation::REVOCATION_REGISTRY.lock().is_key_revoked(&item.publisher_key)
    {
        return AuditVerdict::PackageRevoked;
    }

    // 1. Verify AMCP package binary structure and SHA-256 digest
    let pkg = match AmcpPackage::parse_and_verify(&item.raw_amcp) {
        Ok(p) => p,
        Err(_) => return AuditVerdict::SignatureInvalid,
    };

    // 2. Verify Developer HMAC-SHA256 signature
    if !pkg.verify_signature(&item.publisher_key, &item.developer_signature) {
        return AuditVerdict::SignatureInvalid;
    }

    // 3. Pre-flight capability boundary check
    match pkg.assess_risk() {
        CapabilityRisk::Forbidden => AuditVerdict::BlockedForbidden,
        CapabilityRisk::RequiresConsent => AuditVerdict::RequiresUserConsent,
        CapabilityRisk::Safe => AuditVerdict::Approved,
    }
}

