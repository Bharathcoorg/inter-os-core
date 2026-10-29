//! Comprehensive RFC Test Vectors and Verification for TLS 1.3 & Crypto Primitives
//!
//! Covers:
//! - SHA-256 (FIPS PUB 180-4)
//! - HMAC-SHA256 (RFC 4231)
//! - HKDF-SHA256 (RFC 5869)
//! - X25519 (RFC 7748)
//! - ChaCha20, Poly1305, ChaCha20-Poly1305 AEAD (RFC 8439)
//! - X.509 DER Certificate parsing and validation (RFC 5280)
//! - TLS 1.3 Record Framing and Nonce Generation (RFC 8446)
//! - Mock TLS 1.3 Full Handshake & Encrypted Application Data Exchange
//! - Post-handshake records on the read path (RFC 8446 4.6.1)

use interos_std::crypto::chacha20_poly1305::{
    chacha20_block, ChaCha20Poly1305, Poly1305, TAG_LEN,
};
use interos_std::crypto::hkdf::{derive_secret, hkdf_expand, hkdf_expand_label, hkdf_extract};
use interos_std::crypto::hmac::HmacSha256;
use interos_std::crypto::sha256::Sha256;
use interos_std::crypto::x25519::{x25519, x25519_base};
use interos_std::tls::certificate::{CertificateVerifier, ParsedCertificate};
use interos_std::tls::client::{
    MemoryStream, TlsClient, TlsClientSession, TlsConfig, TlsStream,
};
use interos_std::tls::handshake::{
    HandshakeState, HANDSHAKE_CERTIFICATE_VERIFY, HANDSHAKE_CLIENT_HELLO,
    HANDSHAKE_ENCRYPTED_EXTENSIONS, HANDSHAKE_FINISHED, HANDSHAKE_SERVER_HELLO,
};
use interos_std::tls::record::{
    compute_nonce, ContentType, RecordCipher, MAX_PLAINTEXT_LEN, RECORD_HEADER_LEN,
    TLS_LEGACY_VERSION,
};

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let clean: String = hex.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    assert_eq!(clean.len() % 2, 0);
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
        .collect()
}

/// Test-only permissive verifier, defined inside this test target so it exists
/// in no shipped build. It stands in for a trust store during the mock
/// handshake below; it is NOT a claim that anything was verified, and the
/// transport still enforces the reference identity, the advertised scheme list,
/// message order, and the leaf key pinning independently of it.
struct MockTrustStoreVerifier {
    identity: &'static str,
}

impl CertificateVerifier for MockTrustStoreVerifier {
    fn verify_certificate(&self, _cert: &ParsedCertificate) -> Result<(), interos_std::tls::TlsError> {
        Ok(())
    }
    /// Structural only: these fixtures carry no clock, and neither stub can
    /// report a verified handshake anyway (AnyPassVerifier declares no
    /// identity, and the mock store holds a certificate it never signs).
    fn check_validity_window(&self, cert: &ParsedCertificate) -> Result<(), interos_std::tls::TlsError> {
        cert.check_validity()
    }
    fn verify_trust_anchor(&self, _cert: &ParsedCertificate) -> Result<(), interos_std::tls::TlsError> {
        Ok(())
    }
    fn reference_identity(&self) -> Option<&str> {
        Some(self.identity)
    }
    fn verify_server_signature(
        &self,
        _public_key: &[u8],
        _algorithm: u16,
        _context_and_hash: &[u8],
        _signature: &[u8],
    ) -> Result<(), interos_std::tls::TlsError> {
        Ok(())
    }
}

// ==========================================
// 1. SHA-256 Test Vectors
// ==========================================

#[test]
fn test_sha256_rfc_vectors() {
    // Empty string
    let d1 = Sha256::digest(b"");
    assert_eq!(
        d1,
        hex_to_bytes("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855").as_slice()
    );

    // "abc"
    let d2 = Sha256::digest(b"abc");
    assert_eq!(
        d2,
        hex_to_bytes("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").as_slice()
    );

    // 56-byte message
    let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    let d3 = Sha256::digest(msg);
    assert_eq!(
        d3,
        hex_to_bytes("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1").as_slice()
    );
}

// ==========================================
// 2. HMAC-SHA256 Test Vectors (RFC 4231)
// ==========================================

#[test]
fn test_hmac_sha256_rfc4231() {
    // Test Case 1
    let key1 = [0x0bu8; 20];
    let data1 = b"Hi There";
    let tag1 = HmacSha256::compute(&key1, data1);
    assert_eq!(
        tag1,
        hex_to_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7").as_slice()
    );
    assert!(HmacSha256::verify(&key1, data1, &tag1));

    // Test Case 2
    let key2 = b"Jefe";
    let data2 = b"what do ya want for nothing?";
    let tag2 = HmacSha256::compute(key2, data2);
    assert_eq!(
        tag2,
        hex_to_bytes("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843").as_slice()
    );
    assert!(HmacSha256::verify(key2, data2, &tag2));
}

// ==========================================
// 3. HKDF-SHA256 Test Vectors (RFC 5869)
// ==========================================

#[test]
fn test_hkdf_rfc5869_case1() {
    let ikm = hex_to_bytes("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
    let salt = hex_to_bytes("000102030405060708090a0b0c");
    let info = hex_to_bytes("f0f1f2f3f4f5f6f7f8f9");

    let prk = hkdf_extract(&salt, &ikm);
    assert_eq!(
        prk,
        hex_to_bytes("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5").as_slice()
    );

    let mut okm = [0u8; 42];
    hkdf_expand(&prk, &info, &mut okm).unwrap();
    assert_eq!(
        okm.as_slice(),
        hex_to_bytes("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865").as_slice()
    );
}

// ==========================================
// 4. X25519 Test Vectors (RFC 7748)
// ==========================================

#[test]
fn test_x25519_rfc7748_test_vectors() {
    // Vector 1 (Alice & Bob Diffie-Hellman from RFC 7748 Section 6.1)
    let alice_priv_bytes = hex_to_bytes("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
    let mut alice_priv = [0u8; 32];
    alice_priv.copy_from_slice(&alice_priv_bytes);

    let alice_pub = x25519_base(&alice_priv);
    assert_eq!(
        alice_pub,
        hex_to_bytes("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a").as_slice()
    );

    let bob_priv_bytes = hex_to_bytes("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
    let mut bob_priv = [0u8; 32];
    bob_priv.copy_from_slice(&bob_priv_bytes);

    let bob_pub = x25519_base(&bob_priv);
    assert_eq!(
        bob_pub,
        hex_to_bytes("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f").as_slice()
    );

    let shared_alice = x25519(&alice_priv, &bob_pub);
    let shared_bob = x25519(&bob_priv, &alice_pub);
    let expected_shared = hex_to_bytes("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

    assert_eq!(shared_alice, expected_shared.as_slice());
    assert_eq!(shared_bob, expected_shared.as_slice());

    // Vector 2 (RFC 7748 Section 5.2)
    let scalar_bytes = hex_to_bytes("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
    let point_bytes = hex_to_bytes("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
    let mut scalar = [0u8; 32];
    let mut point = [0u8; 32];
    scalar.copy_from_slice(&scalar_bytes);
    point.copy_from_slice(&point_bytes);

    let result = x25519(&scalar, &point);
    let expected = hex_to_bytes("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
    assert_eq!(result, expected.as_slice());
}

// ==========================================
// 5. ChaCha20 & Poly1305 Test Vectors (RFC 8439)
// ==========================================

#[test]
fn test_chacha20_block_rfc8439() {
    let key_bytes = hex_to_bytes("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    let nonce_bytes = hex_to_bytes("000000090000004a00000000");
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    key.copy_from_slice(&key_bytes);
    nonce.copy_from_slice(&nonce_bytes);

    let block = chacha20_block(&key, 1, &nonce);
    let expected_prefix = hex_to_bytes("10f1e7e4d13b5915500fdd1fa32071c4");
    assert_eq!(&block[..16], expected_prefix.as_slice());
}

#[test]
fn test_poly1305_rfc8439() {
    let key_bytes = hex_to_bytes("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    let msg = b"Cryptographic Forum Research Group";
    let tag = Poly1305::compute(&key, msg);
    let expected_tag = hex_to_bytes("a8061dc1305136c6c22b8baf0c0127a9");
    assert_eq!(tag, expected_tag.as_slice());
}

#[test]
fn test_chacha20_poly1305_aead_rfc8439() {
    let key_bytes = hex_to_bytes("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let nonce_bytes = hex_to_bytes("070000004041424344454647");
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    key.copy_from_slice(&key_bytes);
    nonce.copy_from_slice(&nonce_bytes);

    let aad = hex_to_bytes("50515253c0c1c2c3c4c5c6c7");
    let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

    let mut ciphertext = vec![0u8; plaintext.len()];
    let mut tag = [0u8; 16];

    ChaCha20Poly1305::encrypt(&key, &nonce, &aad, plaintext, &mut ciphertext, &mut tag);

    let expected_tag = hex_to_bytes("1ae10b594f09e26a7e902ecbd0600691");
    assert_eq!(tag, expected_tag.as_slice());

    // Decrypt
    let mut decrypted = vec![0u8; ciphertext.len()];
    assert!(ChaCha20Poly1305::decrypt(&key, &nonce, &aad, &ciphertext, &tag, &mut decrypted).is_ok());
    assert_eq!(decrypted.as_slice(), plaintext);

    // Tampered tag failure
    let mut bad_tag = tag;
    bad_tag[0] ^= 0x01;
    assert!(ChaCha20Poly1305::decrypt(&key, &nonce, &aad, &ciphertext, &bad_tag, &mut decrypted).is_err());
}

// ==========================================
// 6. X.509 DER Certificate Parsing & Validation
// ==========================================

#[test]
fn test_der_certificate_parsing_and_verification() {
    // 201-byte valid DER X.509 certificate with Ed25519 SPKI and signature
    let cert_der = hex_to_bytes(
        "3081c6307a020101300506032b65703010300e06035504030c07496e7465724f53301e170d3236303130313030303030305a170d3236313233313233353935395a3010300e06035504030c07496e7465724f53302a300506032b65700321000909090909090909090909090909090909090909090909090909090909090909300506032b657003410055555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555"
    );

    let parsed = ParsedCertificate::parse(&cert_der).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.serial_number, &[1]);
    assert_eq!(parsed.spki_public_key, &[9u8; 32]);
    assert_eq!(parsed.signature_value.len(), 64);
    assert!(parsed.check_validity().is_ok());
}

// ==========================================
// 7. Record Layer & Nonce Computation
// ==========================================

#[test]
fn test_record_layer_round_trip() {
    let key = [0x5au8; 32];
    let iv = [0x3bu8; 12];
    let mut send_cipher = RecordCipher::new(key, iv);
    let mut recv_cipher = RecordCipher::new(key, iv);

    let plaintext = b"Hello, Sovereign InterOS TLS 1.3!";
    let mut record = [0u8; 256];

    let rec_len = send_cipher
        .encrypt_record(ContentType::ApplicationData, plaintext, &mut record)
        .unwrap();

    assert_eq!(record[0], ContentType::ApplicationData as u8);
    assert_eq!(u16::from_be_bytes([record[1], record[2]]), TLS_LEGACY_VERSION);

    let mut decrypted = [0u8; 256];
    let (content_type, dec_len) = recv_cipher
        .decrypt_record(&record[..rec_len], &mut decrypted)
        .unwrap();

    assert_eq!(content_type, ContentType::ApplicationData);
    assert_eq!(&decrypted[..dec_len], plaintext);
}

// ==========================================
// 8. Full Mock TLS 1.3 Handshake & Application Data
// ==========================================

#[test]
fn test_mock_tls13_handshake_and_traffic() {
    // 1. Client setup
    let client_priv = [0x11u8; 32];
    let client_rand = [0x22u8; 32];

    static VERIFIER: MockTrustStoreVerifier = MockTrustStoreVerifier { identity: "InterOS" };
    let config = TlsConfig {
        server_name: Some("InterOS"),
        random: client_rand,
        private_key: client_priv,
        verifier: &VERIFIER,
    };

    let mut client_session = TlsClientSession::new(config);

    // 2. Client emits ClientHello
    let mut ch_record = [0u8; 512];
    let ch_len = client_session.start_handshake(&mut ch_record).unwrap();
    assert!(ch_len > 0);
    assert_eq!(ch_record[0], ContentType::Handshake as u8);
    assert_eq!(ch_record[5], HANDSHAKE_CLIENT_HELLO);

    // 3. Server generates its ephemeral key and replies with ServerHello
    let server_priv = [0x33u8; 32];
    let server_pub = x25519_base(&server_priv);

    let mut server_hello_body = Vec::new();
    server_hello_body.extend_from_slice(&0x0303u16.to_be_bytes()); // legacy_version
    server_hello_body.extend_from_slice(&[0x77u8; 32]); // random
    server_hello_body.push(32); // session_id length
    server_hello_body.extend_from_slice(&client_rand); // echo session_id
    server_hello_body.extend_from_slice(&0x1303u16.to_be_bytes()); // TLS_CHACHA20_POLY1305_SHA256
    server_hello_body.push(0); // compression method 0

    // Extensions: supported_versions (0x002b) + key_share (0x0033)
    let mut sh_ext = Vec::new();
    // supported_versions
    sh_ext.extend_from_slice(&0x002bu16.to_be_bytes());
    sh_ext.extend_from_slice(&2u16.to_be_bytes());
    sh_ext.extend_from_slice(&0x0304u16.to_be_bytes());
    // key_share
    sh_ext.extend_from_slice(&0x0033u16.to_be_bytes());
    sh_ext.extend_from_slice(&36u16.to_be_bytes()); // 2 + 2 + 32
    sh_ext.extend_from_slice(&0x001du16.to_be_bytes()); // x25519
    sh_ext.extend_from_slice(&32u16.to_be_bytes());
    sh_ext.extend_from_slice(&server_pub);

    server_hello_body.extend_from_slice(&(sh_ext.len() as u16).to_be_bytes());
    server_hello_body.extend_from_slice(&sh_ext);

    let mut sh_msg = Vec::new();
    sh_msg.push(HANDSHAKE_SERVER_HELLO);
    sh_msg.push((server_hello_body.len() >> 16) as u8);
    sh_msg.push((server_hello_body.len() >> 8) as u8);
    sh_msg.push((server_hello_body.len() & 0xff) as u8);
    sh_msg.extend_from_slice(&server_hello_body);

    let mut sh_record = Vec::new();
    sh_record.push(ContentType::Handshake as u8);
    sh_record.extend_from_slice(&0x0303u16.to_be_bytes());
    sh_record.extend_from_slice(&(sh_msg.len() as u16).to_be_bytes());
    sh_record.extend_from_slice(&sh_msg);

    // 4. Client processes ServerHello
    client_session.handle_server_hello(&sh_record).unwrap();

    // 5. Server calculates handshake keys and encrypts EncryptedExtensions, Certificate, CertificateVerify, Finished
    let client_pub = x25519_base(&client_priv);
    let dhe = x25519(&server_priv, &client_pub);

    let early_secret = hkdf_extract(&[0u8; 32], &[0u8; 32]);
    let empty_h = Sha256::digest(&[]);
    let derived_early = derive_secret(&early_secret, b"derived", &empty_h);
    let handshake_secret = hkdf_extract(&derived_early, &dhe);

    // Server tracks transcript
    let mut server_transcript = Sha256::new();
    server_transcript.update(&ch_record[5..ch_len]);
    server_transcript.update(&sh_msg);

    let th1 = server_transcript.finalize();
    let s_hs_traffic = derive_secret(&handshake_secret, b"s hs traffic", &th1);

    let mut s_hs_key = [0u8; 32];
    let mut s_hs_iv = [0u8; 12];
    hkdf_expand_label(&s_hs_traffic, b"key", &[], &mut s_hs_key).unwrap();
    hkdf_expand_label(&s_hs_traffic, b"iv", &[], &mut s_hs_iv).unwrap();

    let mut server_send_cipher = RecordCipher::new(s_hs_key, s_hs_iv);

    // EncryptedExtensions
    let ee_msg = [HANDSHAKE_ENCRYPTED_EXTENSIONS, 0, 0, 2, 0, 0];
    server_transcript.update(&ee_msg);
    let mut ee_record = [0u8; 256];
    let ee_len = server_send_cipher
        .encrypt_record(ContentType::Handshake, &ee_msg, &mut ee_record)
        .unwrap();
    client_session.handle_encrypted_handshake_record(&ee_record[..ee_len]).unwrap();

    // Certificate (valid 201-byte X.509 DER Certificate)
    let cert_msg = hex_to_bytes(
        "0b0000d2000000ce0000c93081c6307a020101300506032b65703010300e06035504030c07496e7465724f53301e170d3236303130313030303030305a170d3236313233313233353935395a3010300e06035504030c07496e7465724f53302a300506032b65700321000909090909090909090909090909090909090909090909090909090909090909300506032b6570034100555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555550000"
    );
    server_transcript.update(&cert_msg);
    let mut cert_record = [0u8; 512];
    let cert_rec_len = server_send_cipher
        .encrypt_record(ContentType::Handshake, &cert_msg, &mut cert_record)
        .unwrap();
    client_session.handle_encrypted_handshake_record(&cert_record[..cert_rec_len]).unwrap();

    // CertificateVerify
    let cv_payload = [
        HANDSHAKE_CERTIFICATE_VERIFY,
        0, 0, 6,
        0x08, 0x07, // ed25519
        0, 2,       // sig len = 2
        0xaa, 0xbb, // dummy signature
    ];
    server_transcript.update(&cv_payload);
    let mut cv_record = [0u8; 256];
    let cv_rec_len = server_send_cipher
        .encrypt_record(ContentType::Handshake, &cv_payload, &mut cv_record)
        .unwrap();
    client_session.handle_encrypted_handshake_record(&cv_record[..cv_rec_len]).unwrap();

    // Server Finished
    let mut s_finished_key = [0u8; 32];
    hkdf_expand_label(&s_hs_traffic, b"finished", &[], &mut s_finished_key).unwrap();
    let th_fin = server_transcript.finalize();
    let s_verify_data = HmacSha256::compute(&s_finished_key, &th_fin);

    let mut sf_msg = [0u8; 36];
    sf_msg[0] = HANDSHAKE_FINISHED;
    sf_msg[1] = 0;
    sf_msg[2] = 0;
    sf_msg[3] = 32;
    sf_msg[4..36].copy_from_slice(&s_verify_data);

    server_transcript.update(&sf_msg);
    let mut sf_record = [0u8; 256];
    let sf_rec_len = server_send_cipher
        .encrypt_record(ContentType::Handshake, &sf_msg, &mut sf_record)
        .unwrap();
    client_session.handle_encrypted_handshake_record(&sf_record[..sf_rec_len]).unwrap();

    // 6. Client emits Finished
    let mut client_fin_record = [0u8; 256];
    let client_fin_len = client_session.emit_client_finished(&mut client_fin_record).unwrap();
    assert!(client_fin_len > 0);
    assert!(client_session.is_connected());

    // 7. Verify encrypted Application Data exchange
    let request_data = b"GET / HTTP/1.1\r\nHost: InterOS\r\n\r\n";
    let mut app_record = [0u8; 256];
    let app_len = client_session.encrypt_application_data(request_data, &mut app_record).unwrap();

    // Server decrypts client application data:
    let derived_hs = derive_secret(&handshake_secret, b"derived", &empty_h);
    let master_secret = hkdf_extract(&derived_hs, &[0u8; 32]);
    let th_app = server_transcript.finalize();
    let c_ap_traffic = derive_secret(&master_secret, b"c ap traffic", &th_app);

    let mut c_ap_key = [0u8; 32];
    let mut c_ap_iv = [0u8; 12];
    hkdf_expand_label(&c_ap_traffic, b"key", &[], &mut c_ap_key).unwrap();
    hkdf_expand_label(&c_ap_traffic, b"iv", &[], &mut c_ap_iv).unwrap();

    let mut server_recv_app_cipher = RecordCipher::new(c_ap_key, c_ap_iv);
    let mut decrypted_req = [0u8; 256];
    let (c_type, dec_req_len) = server_recv_app_cipher
        .decrypt_record(&app_record[..app_len], &mut decrypted_req)
        .unwrap();

    assert_eq!(c_type, ContentType::ApplicationData);
    assert_eq!(&decrypted_req[..dec_req_len], request_data);
}

// ==========================================
// 6. Post-handshake records on the read path
// ==========================================

/// The keys the mock peer encrypts its post-handshake records under, and the
/// keys the client under test is given as its receive keys.
const PEER_APP_KEY: [u8; 32] = [0x6eu8; 32];
const PEER_APP_IV: [u8; 12] = [0x74u8; 12];

/// A `NewSessionTicket` message body of the given length.
///
/// The filler is deliberately non-zero: the record layer finds the inner content
/// type by scanning backwards past trailing zero bytes, so a body of zeros would
/// be shorter than the ticket this is meant to stand in for.
fn new_session_ticket(body_len: usize) -> Vec<u8> {
    let mut msg = vec![4u8, 0, 0, body_len as u8];
    msg.resize(4 + body_len, 0xa5);
    msg
}

/// Encrypts a sequence of post-handshake records the way a server would, so the
/// client's receive cipher sees the same sequence numbers it would on the wire.
///
/// WHY the peer seals with the AEAD directly instead of through
/// `RecordCipher::encrypt_record`: this client's own sealing path frames one
/// request-sized record, while a server fills a record to the protocol limit,
/// and a test peer that could only make small records could not produce the
/// answer the reader has to cope with.
fn peer_records(items: &[(ContentType, Vec<u8>)]) -> Vec<u8> {
    let mut wire = Vec::new();
    for (seq, (kind, plaintext)) in items.iter().enumerate() {
        let nonce = compute_nonce(&PEER_APP_IV, seq as u64);
        let mut inner = plaintext.clone();
        inner.push(*kind as u8);

        let mut header = [0u8; RECORD_HEADER_LEN];
        header[0] = ContentType::ApplicationData as u8;
        header[1..3].copy_from_slice(&TLS_LEGACY_VERSION.to_be_bytes());
        let record_len = inner.len() + TAG_LEN;
        header[3..5].copy_from_slice(&(record_len as u16).to_be_bytes());

        let mut ciphertext = vec![0u8; inner.len()];
        let mut tag = [0u8; TAG_LEN];
        ChaCha20Poly1305::encrypt(
            &PEER_APP_KEY,
            &nonce,
            &header,
            &inner,
            &mut ciphertext,
            &mut tag,
        );

        wire.extend_from_slice(&header);
        wire.extend_from_slice(&ciphertext);
        wire.extend_from_slice(&tag);
    }
    wire
}

/// A client that has completed the handshake and holds the keys
/// [`peer_records`] encrypts under.
///
/// WHY the state is placed directly rather than reached through the handshake:
/// what is under test is the record reader, and re-running the key schedule
/// would prove nothing about it that the mock handshake above does not already
/// prove. The application receive cipher is a public field so the record layer
/// can be exercised on its own.
fn client_awaiting_records<'w>(
    wire: &'w [u8],
    sink: &'w mut [u8],
) -> TlsClient<'w, MemoryStream<'w>> {
    let mut client = TlsClient::new(MemoryStream::new(wire, sink), TlsConfig::default());
    client.session.handshake.state = HandshakeState::Connected;
    client.session.app_recv_cipher = Some(RecordCipher::new(PEER_APP_KEY, PEER_APP_IV));
    client
}

/// RFC 8446 4.6.1: a server sends one or more `NewSessionTicket` records as soon
/// as the handshake completes, so the first record answering a GET is a ticket,
/// not the document. Reading it as a fault ended the fetch with zero bytes and
/// the browser then reported that the gateway had returned nothing.
#[test]
fn a_ticket_before_the_document_still_delivers_the_document() {
    let document = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    let wire = peer_records(&[
        (ContentType::Handshake, new_session_ticket(64)),
        (ContentType::ApplicationData, document.to_vec()),
    ]);

    let mut sink = [0u8; 512];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 512];
    let read = client.read(&mut out).unwrap();
    assert_eq!(&out[..read], &document[..]);

    // The peer closes after the response: the next read is the end of the
    // stream, which the reader reports as a zero-length read rather than a fault.
    assert_eq!(client.read(&mut out).unwrap(), 0);
}

/// WHY a discarded record does not use the caller's buffer: a caller asking for
/// eight bytes has said how much of the document it can hold, not how much of
/// the peer's housekeeping it has to tolerate. Sharing the window made a long
/// ticket refuse the fetch outright.
#[test]
fn a_ticket_larger_than_the_callers_window_does_not_eat_the_document() {
    let document = b"hello";
    let wire = peer_records(&[
        (ContentType::Handshake, new_session_ticket(400)),
        (ContentType::ApplicationData, document.to_vec()),
    ]);

    let mut sink = [0u8; 512];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 8];
    let read = client.read(&mut out).unwrap();
    assert_eq!(&out[..read], &document[..]);
}

/// A post-handshake message that changes the keys cannot be stepped over: the
/// records after it would decrypt to garbage under the old keys, and handing
/// that garbage to the reader would present a forged stream as the peer's answer.
#[test]
fn a_post_handshake_record_that_is_not_a_ticket_is_refused() {
    let key_update = vec![24u8, 0, 0, 1, 1u8];
    let wire = peer_records(&[(ContentType::Handshake, key_update)]);

    let mut sink = [0u8; 512];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 512];
    assert_eq!(
        client.read(&mut out),
        Err(interos_std::tls::TlsError::UnexpectedMessage)
    );
}

/// An alert is the peer saying it stopped, which is a fact the caller has to be
/// told about rather than one that arrives as an empty read.
#[test]
fn an_alert_record_is_refused_rather_than_read_as_an_empty_page() {
    let wire = peer_records(&[(ContentType::Alert, vec![2u8, 0u8])]);

    let mut sink = [0u8; 512];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 512];
    assert_eq!(
        client.read(&mut out),
        Err(interos_std::tls::TlsError::UnexpectedMessage)
    );
}

/// Tickets are skipped because they are the peer's business, but a peer that
/// sends nothing else would otherwise keep the read spinning forever. The bound
/// turns that into a named refusal instead of a hang or a zero-byte page.
#[test]
fn a_peer_that_only_sends_tickets_is_refused_rather_than_read_as_an_empty_page() {
    let mut items = Vec::new();
    for _ in 0..32 {
        items.push((ContentType::Handshake, new_session_ticket(16)));
    }
    let wire = peer_records(&items);

    let mut sink = [0u8; 512];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 512];
    assert_eq!(
        client.read(&mut out),
        Err(interos_std::tls::TlsError::UnexpectedMessage)
    );
}

/// A peer that only sends tickets is refused, but the other half of the same
/// defect was size: a server puts the first kilobytes of a page inside one
/// record, and a reader that framed 2 KiB refused the answer to its own request.
/// The record now arrives whole through a window far smaller than it is.
#[test]
fn a_record_larger_than_the_callers_window_arrives_whole() {
    let mut document = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n".to_vec();
    document.extend_from_slice(&vec![b'x'; 9000]);
    let expected = document.clone();
    let wire = peer_records(&[(ContentType::ApplicationData, document)]);

    let mut sink = [0u8; 64];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut received = Vec::new();
    let mut reads = 0usize;
    loop {
        let mut out = [0u8; 2048];
        let read = client.read(&mut out).unwrap();
        if read == 0 {
            break;
        }
        received.extend_from_slice(&out[..read]);
        reads += 1;
        assert!(reads < 10, "one record took {reads} reads to hand over");
    }

    assert_eq!(received, expected);
    assert!(reads > 1, "a 9 KiB record was handed over in one read");
}

/// The protocol's own ceiling, not a number this client happened to pick.
///
/// WHY the exact size: a record filled to `MAX_PLAINTEXT_LEN` is what a CDN
/// sends when it has a page to deliver, and an off-by-one in the frame the
/// reader holds only shows up at that boundary.
///
/// WHY a whole fragment rather than one byte less: measured on the wire against
/// a live OpenSSL server, an answer written in 16 KiB pieces arrives as a record
/// whose length field is 16401 — the fragment, the content type byte that is
/// sealed with it, and the tag. A reader that sized its frame by the fragment
/// alone is one byte short of the biggest thing a real server sends.
#[test]
fn the_biggest_record_the_protocol_allows_is_read_whole() {
    let document = vec![b'A'; MAX_PLAINTEXT_LEN];
    let wire = peer_records(&[(ContentType::ApplicationData, document.clone())]);

    let mut sink = [0u8; 64];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut received = Vec::new();
    loop {
        let mut out = [0u8; 4096];
        let read = client.read(&mut out).unwrap();
        if read == 0 {
            break;
        }
        received.extend_from_slice(&out[..read]);
    }

    assert_eq!(received, document);
}

/// How many bytes the device's own transport is willing to hand over in one call.
///
/// WHY the number is the kernel's rather than the reader's: `tcp_recv` answers
/// `InvalidArguments` above this, so a TLS reader that asks the wire for a whole
/// record is refused by the machine it runs on.
const TRANSPORT_WINDOW: usize = 2048;

/// A transport that refuses any read larger than the device allows.
///
/// WHY this stands beside `MemoryStream`: the memory stream answers a read of any
/// size, so every test built on it proved the reader could hold a page-sized
/// record without ever proving it could *collect* one. On the phone the
/// 126-byte answer arrived and the 16 KiB one did not, and the difference was
/// this cap.
struct CappedStream<'a> {
    inner: MemoryStream<'a>,
}

impl<'a> TlsStream for CappedStream<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, interos_std::tls::TlsError> {
        if buf.len() > TRANSPORT_WINDOW {
            return Err(interos_std::tls::TlsError::IoError);
        }
        self.inner.read(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), interos_std::tls::TlsError> {
        self.inner.write_all(buf)
    }
}

/// A page-sized record arrives whole through a transport that hands out 2 KiB a
/// call, which is the only size the device's TCP receive allows.
#[test]
fn a_page_sized_record_is_gathered_from_a_transport_that_hands_out_two_kib() {
    let document = vec![b'B'; MAX_PLAINTEXT_LEN];
    let wire = peer_records(&[(ContentType::ApplicationData, document.clone())]);

    let mut sink = [0u8; 64];
    let mut client = TlsClient::new(
        CappedStream {
            inner: MemoryStream::new(&wire, &mut sink),
        },
        TlsConfig::default(),
    );
    client.session.handshake.state = HandshakeState::Connected;
    client.session.app_recv_cipher = Some(RecordCipher::new(PEER_APP_KEY, PEER_APP_IV));

    let mut received = Vec::new();
    loop {
        let mut out = [0u8; 4096];
        let read = client.read(&mut out).unwrap();
        if read == 0 {
            break;
        }
        received.extend_from_slice(&out[..read]);
    }

    assert_eq!(received, document);
}

/// WHY the reader stops at the protocol's limit rather than at its own: a peer
/// that claims a longer record than TLS allows is describing bytes this client
/// will never hold, so the fetch has to end in a named refusal. Reading on would
/// hand the caller a page that was cut in the middle.
#[test]
fn a_record_beyond_the_protocol_limit_is_refused() {
    let claimed = (MAX_PLAINTEXT_LEN + TAG_LEN + 100) as u16;
    let mut wire = vec![ContentType::ApplicationData as u8, 3, 3];
    wire.extend_from_slice(&claimed.to_be_bytes());
    wire.extend_from_slice(&[0u8; 64]);

    let mut sink = [0u8; 64];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 2048];
    assert_eq!(
        client.read(&mut out),
        Err(interos_std::tls::TlsError::RecordTooLarge)
    );
}

/// An empty record is the peer still talking, not the peer having stopped.
///
/// WHY it matters here: `Ok(0)` is the reader's only word for the end of the
/// stream and a fetch that sees it stops asking, so an empty record taken for an
/// end would cut a response short with the rest of the page still on the wire.
#[test]
fn an_empty_application_data_record_does_not_end_the_response() {
    let wire = peer_records(&[
        (ContentType::ApplicationData, Vec::new()),
        (ContentType::ApplicationData, b"hello".to_vec()),
    ]);

    let mut sink = [0u8; 64];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 64];
    let read = client.read(&mut out).unwrap();
    assert_eq!(&out[..read], b"hello");
}

/// WHAT a failed tag is allowed to produce: nothing.
///
/// WHY this is a test and not an assumption: opening a record in place means the
/// bytes the caller receives came out of the same buffer the ciphertext arrived
/// in, so the check has to happen before any of it is unmasked. A tampered
/// record must leave the caller's window exactly as it was handed over.
#[test]
fn a_record_that_does_not_authenticate_releases_no_bytes() {
    let mut wire = peer_records(&[(ContentType::ApplicationData, b"the page".to_vec())]);
    let last = wire.len() - 1;
    wire[last] ^= 0x01; // the tag no longer describes the record

    let mut sink = [0u8; 64];
    let mut client = client_awaiting_records(&wire, &mut sink);
    let mut out = [0u8; 64];
    assert_eq!(
        client.read(&mut out),
        Err(interos_std::tls::TlsError::DecryptionFailed)
    );
    assert_eq!(out, [0u8; 64], "an unauthenticated record reached the caller");
}

/// The mock store above is a test fixture, not a trust decision: the client
/// refuses to run any handshake whose advertised name is not the name the
/// verifier pins, so a permissive stub can never carry a session to `Connected`
/// under a borrowed identity.
#[test]
fn test_stub_verifier_cannot_borrow_another_identity() {
    static MISMATCH: MockTrustStoreVerifier = MockTrustStoreVerifier { identity: "someone.else" };
    let mut session = TlsClientSession::new(TlsConfig {
        server_name: Some("InterOS"),
        random: [0x22u8; 32],
        private_key: [0x11u8; 32],
        verifier: &MISMATCH,
    });
    let mut ch_record = [0u8; 1024];
    assert_eq!(
        session.start_handshake(&mut ch_record),
        Err(interos_std::tls::TlsError::Unavailable)
    );
    assert!(!session.is_connected());

    // And TlsConfig::new refuses the same mismatch up front.
    assert!(
        TlsConfig::new("InterOS", [0x22u8; 32], [0x11u8; 32], &MISMATCH).is_err()
    );
}

/// A shipped consumer must find NO trust anchor and NO mock CA.
///
/// This test target compiles against the library *without* `cfg(test)`, so it
/// sees exactly what a guest image sees. The synthetic bootstrap CA is a
/// fixture (placeholder key, dummy signature) and must not be reachable from
/// a production build: the shipped trust store is empty, so a verifier that
/// was not given real roots fails closed instead of reporting "verified".
#[test]
fn test_shipped_build_has_no_trust_anchor_and_no_mock_ca() {
    use interos_std::tls::certificate::DEFAULT_TRUSTED_ROOTS;
    use interos_std::tls::DefaultCertVerifier;

    assert!(
        DEFAULT_TRUSTED_ROOTS.is_empty(),
        "a shipped client must start with no trust anchor"
    );

    // The synthetic InterOS bootstrap CA, byte for byte, as it used to be the
    // shipped default. A default verifier must refuse it.
    let synthetic_root = hex_to_bytes(
        "3081c6307a020101300506032b65703010300e06035504030c07496e7465724f53301e170d3236303130313030303030305a170d3236313233313233353935395a3010300e06035504030c07496e7465724f53302a300506032b65700321000909090909090909090909090909090909090909090909090909090909090909300506032b657003410055555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555555"
    );
    let cert = ParsedCertificate::parse(&synthetic_root).unwrap();
    let verifier = DefaultCertVerifier::for_host("InterOS");
    assert_eq!(
        verifier.verify_certificate(&cert),
        Err(interos_std::tls::TlsError::CertificateVerificationFailed),
        "the mock CA must never be reported as verified"
    );
    assert_eq!(
        verifier.verify_trust_anchor(&cert),
        Err(interos_std::tls::TlsError::CertificateVerificationFailed)
    );
}
