//! Package Management & Execution Subsystem
//!
//! Handles verified loading, cryptographic verification, and revocation
//! of software packages before execution.

pub mod amcp;
pub mod revocation;

pub use amcp::{AmcpPackage, CapabilityRisk, PackageError, ResourceLimits, AMCP_MAGIC, AMCP_VERSION};
pub use revocation::{RevocationReason, RevocationRecord, RevocationRegistry, KeyRotationProof, REVOCATION_REGISTRY};
