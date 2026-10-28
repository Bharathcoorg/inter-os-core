//! Pure `no_std` On-Device TLS 1.3 Client (RFC 8446)
//!
//! Provides sovereign userspace TLS 1.3 encryption and authentication:
//! - Cipher suite: `TLS_CHACHA20_POLY1305_SHA256` (0x1303)
//! - Key exchange: `x25519` (0x001d)
//! - Handshake state machine: ClientHello -> ServerHello -> EncryptedExtensions -> Certificate -> CertificateVerify -> Finished
//! - DER Certificate parser & validator
//! - Bounded record layer with AEAD encryption & decryption
//! Pure Rust, zero third-party dependencies, zero heap allocations.

pub mod certificate;
pub mod client;
pub mod handshake;
pub mod record;

pub use certificate::{
    AllowAnyCertVerifier, CertificateVerifier, DefaultCertVerifier, ParsedCertificate,
    SIG_SCHEME_ED25519, SIG_SCHEME_MOCK_TEST,
};
pub use client::{MemoryStream, TlsClient, TlsClientSession, TlsConfig, TlsStream};
pub use handshake::{
    HandshakeClient, HandshakeState, TLS_CIPHER_CHACHA20_POLY1305_SHA256, TLS_VERSION_1_3,
};
pub use record::{
    compute_nonce, ContentType, RecordCipher, MAX_PLAINTEXT_LEN, RECORD_HEADER_LEN,
    TLS_LEGACY_VERSION,
};

/// TLS 1.3 Error conditions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TlsError {
    InvalidContentType,
    InvalidRecordVersion,
    BadRecordLength,
    RecordTooLarge,
    BufferTooSmall,
    DecryptionFailed,
    BadHandshakeMessage,
    UnexpectedMessage,
    UnsupportedCipherSuite,
    UnsupportedVersion,
    CertificateParsingError,
    CertificateVerificationFailed,
    InvalidSignature,
    VerifyDataMismatch,
    IoError,
    KeyDerivationError,
}
