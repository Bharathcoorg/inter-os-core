//! Package Management & Execution Subsystem
//!
//! Handles verified loading, cryptographic verification, and revocation
//! of software packages before execution.

pub mod rapp;
pub mod revocation;

pub use rapp::{
    build_rapp, parse_rapp, RappArch, RappError, RappManifest, RAPP_FORMAT_VERSION, RAPP_HEADER_SIZE,
    RAPP_MAGIC,
};
pub use revocation::{
    KeyRotationProof, RevocationReason, RevocationRecord, RevocationRegistry, REVOCATION_REGISTRY,
};
