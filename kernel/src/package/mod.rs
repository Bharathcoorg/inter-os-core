//! Package Management & Execution Subsystem
//!
//! Handles verified loading and policy compliance of `.amcp`
//! automation packages before execution by the agent runtime.

pub mod amcp;

pub use amcp::{AmcpPackage, CapabilityRisk, PackageError, ResourceLimits, AMCP_MAGIC, AMCP_VERSION};
