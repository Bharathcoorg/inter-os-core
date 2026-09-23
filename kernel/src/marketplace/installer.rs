//! Dynamic AMCP Package Installer
//!
//! Enforces pre-flight capability audits and publisher cryptographic signatures
//! before committing automation packages to persistent storage under `/packages/`.

use alloc::format;
use alloc::string::String;
use crate::fs::VFS;
use super::catalog::MARKETPLACE_CATALOG;
use super::audit::{audit_item, AuditVerdict};

/// Error states encountered during package installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    PackageNotFound,
    AuditFailedForbidden,
    AuditFailedRequiresConsent,
    SignatureInvalid,
    FsError,
}

/// Installs a verified marketplace package onto the persistent VFS filesystem.
pub fn install_package(package_id: &str, user_consent: bool) -> Result<String, InstallError> {
    let catalog = MARKETPLACE_CATALOG.lock();
    let item = catalog.get(package_id).ok_or(InstallError::PackageNotFound)?;

    // 1. Run pre-flight security and signature audit
    match audit_item(item) {
        AuditVerdict::BlockedForbidden => return Err(InstallError::AuditFailedForbidden),
        AuditVerdict::SignatureInvalid => return Err(InstallError::SignatureInvalid),
        AuditVerdict::RequiresUserConsent if !user_consent => {
            return Err(InstallError::AuditFailedRequiresConsent);
        }
        _ => {}
    }

    // 2. Construct destination path: /packages/<package_id>.amcp
    let dest_path = format!("/packages/{}.amcp", item.package_id);

    // 3. Commit package binary to VFS
    let mut vfs = VFS.lock();
    vfs.create_file(&dest_path, &item.raw_amcp, false);

    Ok(dest_path)
}
