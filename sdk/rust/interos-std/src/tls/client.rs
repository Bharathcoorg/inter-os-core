// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! TLS 1.3 Client Stream and Session (RFC 8446)
//!
//! Provides `TlsClient<S>` wrapping any byte transport stream implementing `TlsStream`,
//! as well as sans-I/O `TlsClientSession` for direct buffer processing. Pure `#![no_std]`.

use super::certificate::{
    is_ip_reference, CertificateVerifier, DefaultCertVerifier, ParsedCertificate,
    MAX_REFERENCE_IDENTITY_LEN,
};
use super::handshake::{
    HandshakeClient, HandshakeState, HANDSHAKE_CERTIFICATE, HANDSHAKE_CERTIFICATE_VERIFY,
    HANDSHAKE_ENCRYPTED_EXTENSIONS, HANDSHAKE_FINISHED,
};
use super::record::{ContentType, MAX_RECORD_WIRE_LEN, RecordCipher, RECORD_HEADER_LEN};
use super::TlsError;

/// The `HandshakeType` of a post-handshake `NewSessionTicket` (RFC 8446 4.6.1).
///
/// WHY it is not with the other handshake types: those name the messages this
/// client walks during the handshake, and this one exists only so the reader
/// after the handshake can recognise it and drop it.
const HANDSHAKE_NEW_SESSION_TICKET: u8 = 4;

/// How many records that are not the document one read will step over.
///
/// WHY a bound at all: a peer may send as many tickets as it likes, and a fetch
/// that skipped them without limit would never return. Eight is well past what a
/// server sends unprompted, and passing it is reported as a refusal rather than
/// as an empty page.
const MAX_SKIPPED_RECORDS: usize = 8;

/// The largest record this client writes at once, header and tag included.
///
/// WHY it is smaller than what the client can read: a request is a few hundred
/// bytes, and `RecordCipher::encrypt_record` frames the plaintext it was handed
/// into a buffer of this size. Growing it would spend the same stack on a write
/// path that never needs the room.
const MAX_RECORD_BYTES: usize = 2048;

/// The largest record the read path can hold, header and tag included.
///
/// WHY it follows the protocol rather than this client's convenience: a server
/// fills a record to `MAX_PLAINTEXT_LEN` and seals the content type with it, so
/// a reader that frames less refuses the answer to its own request. One record
/// is what a page begins with.
const MAX_READ_RECORD_BYTES: usize = MAX_RECORD_WIRE_LEN;

/// How many bytes one call to the transport is asked for.
///
/// WHY the reader does not ask for the rest of the record at once: the device's
/// TCP receive answers `InvalidArguments` above its own window, so a reader that
/// requests a whole page-sized record is refused by the machine it runs on. The
/// record is still gathered whole into `record` -- only the request is bounded.
const TRANSPORT_READ_BYTES: usize = 2048;

/// The largest handshake message a [`HandshakeReader`] will rejoin.
///
/// WHY 32 KiB: the chain a public server sends is the message that decides this.
/// Measured on the wire, `developer.mozilla.org` writes a 4113-byte certificate
/// message and `example.com` a 3686-byte one, while large modern hosts with multiple
/// intermediate certificates, SCT extensions, and OCSP stapling can reach 16-24 KiB.
/// 32 KiB accommodates these chains while remaining strictly bounded.
pub const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 32768;

/// Handshake messages rejoined across the record boundaries a server chose.
///
/// WHY this exists at all: RFC 8446 5.1 lets a server put several handshake
/// messages in one record and carry one message over several records, and a
/// certificate chain is larger than the record a server is required to fit it
/// into. A reader that calls the first byte of a record the message type is then
/// right only by luck, and every public chain measured here is big enough to
/// make that luck run out.
///
/// WHY the caller brings the buffer: this is the room a chain takes, and which
/// client asks for it is the client's memory budget, not the protocol's.
pub struct HandshakeReader<'a> {
    buf: &'a mut [u8],
    /// Bytes held that no message has been cut from yet.
    held: usize,
}

impl<'a> HandshakeReader<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, held: 0 }
    }

    /// The room still free, so a record's payload can be decrypted straight
    /// into it instead of through a second frame.
    pub fn tail(&mut self) -> &mut [u8] {
        &mut self.buf[self.held..]
    }

    /// Claims `written` bytes of the tail as arriving record payload.
    pub fn commit(&mut self, written: usize) -> Result<(), TlsError> {
        if written > self.buf.len() - self.held {
            return Err(TlsError::BufferTooSmall);
        }
        self.held += written;
        Ok(())
    }

    /// Hands every whole message to `f`, oldest first, and keeps only the prefix
    /// of a message that has not arrived in full.
    ///
    /// WHY `f` is called before the remainder moves: the caller holds `&mut` to
    /// the session it is feeding, and a message is only ever complete, so no
    /// dispatch has to be unwound if a later one fails.
    pub fn drain<F>(&mut self, mut f: F) -> Result<(), TlsError>
    where
        F: FnMut(&[u8]) -> Result<(), TlsError>,
    {
        let total = self.held;
        let mut taken = 0usize;
        while total - taken >= 4 {
            let declared = (self.buf[taken + 1] as usize) << 16
                | (self.buf[taken + 2] as usize) << 8
                | self.buf[taken + 3] as usize;
            let message_len = match 4usize.checked_add(declared) {
                Some(len) => len,
                None => return Err(TlsError::BadHandshakeMessage),
            };
            if message_len > MAX_HANDSHAKE_MESSAGE_BYTES {
                return Err(TlsError::RecordTooLarge);
            }
            if message_len > total - taken {
                // Only a prefix of this message has arrived; the next record
                // carries the rest, and the bytes already held stay where they
                // are until a whole message can be cut.
                break;
            }
            f(&self.buf[taken..taken + message_len])?;
            taken += message_len;
        }
        self.buf.copy_within(taken..total, 0);
        self.held = total - taken;
        Ok(())
    }
}

/// Stream abstraction for underlying transport (e.g. TCP socket, mock memory buffer).
pub trait TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError>;
    fn write_all(&mut self, buf: &[u8]) -> Result<(), TlsError>;
}

/// In-memory byte buffer implementation of `TlsStream` for unit testing and mock exchanges.
pub struct MemoryStream<'a> {
    pub read_buf: &'a [u8],
    pub read_pos: usize,
    pub write_buf: &'a mut [u8],
    pub write_pos: usize,
}

impl<'a> MemoryStream<'a> {
    pub fn new(read_slice: &'a [u8], write_slice: &'a mut [u8]) -> Self {
        Self {
            read_buf: read_slice,
            read_pos: 0,
            write_buf: write_slice,
            write_pos: 0,
        }
    }

    pub fn written_data(&self) -> &[u8] {
        &self.write_buf[..self.write_pos]
    }
}

impl<'a> TlsStream for MemoryStream<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        let available = self.read_buf.len() - self.read_pos;
        if available == 0 {
            return Ok(0);
        }
        let to_copy = buf.len().min(available);
        buf[..to_copy].copy_from_slice(&self.read_buf[self.read_pos..self.read_pos + to_copy]);
        self.read_pos += to_copy;
        Ok(to_copy)
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), TlsError> {
        let available = self.write_buf.len() - self.write_pos;
        if buf.len() > available {
            return Err(TlsError::BufferTooSmall);
        }
        self.write_buf[self.write_pos..self.write_pos + buf.len()].copy_from_slice(buf);
        self.write_pos += buf.len();
        Ok(())
    }
}

/// Configuration for TLS 1.3 client connection.
pub struct TlsConfig<'a> {
    pub server_name: Option<&'a str>,
    pub random: [u8; 32],
    pub private_key: [u8; 32],
    pub verifier: &'a dyn CertificateVerifier,
}

impl<'a> TlsConfig<'a> {
    /// Creates a validated TLS 1.3 client configuration.
    /// Fails with Unavailable if required entropy, keys, or peer name are missing or static defaults,
    /// or if the verifier does not pin exactly the name this client will advertise in SNI.
    pub fn new(
        server_name: &'a str,
        random: [u8; 32],
        private_key: [u8; 32],
        verifier: &'a dyn CertificateVerifier,
    ) -> Result<Self, TlsError> {
        if server_name.is_empty()
            || server_name.len() > MAX_REFERENCE_IDENTITY_LEN
            || random == [0u8; 32]
            || random == [0x42u8; 32]
            || private_key == [0u8; 32]
            || private_key == [0x24u8; 32]
        {
            return Err(TlsError::Unavailable);
        }
        // What the ClientHello advertises and what the verifier checks must be
        // the same identity, otherwise the handshake proves a name nobody asked for.
        if verifier.reference_identity() != Some(server_name) {
            return Err(TlsError::Unavailable);
        }
        Ok(Self {
            server_name: Some(server_name),
            random,
            private_key,
            verifier,
        })
    }
}

impl<'a> Default for TlsConfig<'a> {
    fn default() -> Self {
        static DEFAULT_VERIFIER: DefaultCertVerifier<'static> = DefaultCertVerifier::new();
        Self {
            server_name: None,
            random: [0u8; 32],
            private_key: [0u8; 32],
            verifier: &DEFAULT_VERIFIER,
        }
    }
}

/// Sans-I/O TLS 1.3 Client Session state machine.
pub struct TlsClientSession<'a> {
    pub handshake: HandshakeClient,
    pub config: TlsConfig<'a>,
    pub send_cipher: Option<RecordCipher>,
    pub recv_cipher: Option<RecordCipher>,
    pub app_send_cipher: Option<RecordCipher>,
    pub app_recv_cipher: Option<RecordCipher>,
    pub last_parsed_cert: Option<ParsedCertificate<'a>>,
    /// The leaf `subjectPublicKey` bytes, pinned from the certificate the client
    /// validated so `CertificateVerify` can only be checked against that key.
    pub server_cert_spki: [u8; crate::tls::certificate::MAX_LEAF_PUBLIC_KEY_BYTES],
    pub server_cert_spki_len: usize,
    pub has_server_cert: bool,
}

impl<'a> TlsClientSession<'a> {
    pub fn new(config: TlsConfig<'a>) -> Self {
        let handshake = HandshakeClient::new(config.private_key);
        Self {
            handshake,
            config,
            send_cipher: None,
            recv_cipher: None,
            app_send_cipher: None,
            app_recv_cipher: None,
            last_parsed_cert: None,
            server_cert_spki: [0u8; crate::tls::certificate::MAX_LEAF_PUBLIC_KEY_BYTES],
            server_cert_spki_len: 0,
            has_server_cert: false,
        }
    }

    /// The reference identity this session is allowed to use, or nothing.
    ///
    /// Enforces, on every live handshake entry point (so a hand-built `TlsConfig`
    /// through the public fields cannot bypass it):
    /// - a non-empty, wire-length peer name is configured,
    /// - the key material is not a static/default value,
    /// - the verifier pins exactly that name, so the identity advertised in the
    ///   ClientHello is the identity the certificate is checked against.
    fn require_live_identity(&self) -> Result<&'a str, TlsError> {
        let host = self.config.server_name.ok_or(TlsError::Unavailable)?;
        if host.is_empty() || host.len() > MAX_REFERENCE_IDENTITY_LEN {
            return Err(TlsError::Unavailable);
        }
        if self.config.private_key == [0u8; 32]
            || self.config.private_key == [0x24u8; 32]
            || self.config.random == [0u8; 32]
            || self.config.random == [0x42u8; 32]
        {
            return Err(TlsError::Unavailable);
        }
        if self.config.verifier.reference_identity() != Some(host) {
            return Err(TlsError::Unavailable);
        }
        Ok(host)
    }

    /// Emits ClientHello into `out`. Returns total record length.
    pub fn start_handshake(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        let host = self.require_live_identity()?;
        // RFC 6066 §3: the `server_name` extension carries a hostname, and a
        // literal IPv4/IPv6 address "must not" be put in it. The identity is
        // still verified against the certificate's iPAddress entries -- that is
        // the `matches_hostname` decision in the handshake walk, which needs no
        // SNI -- but nothing of it is advertised on the wire.
        let sni = if is_ip_reference(host) {
            None
        } else {
            Some(host)
        };
        self.handshake.encode_client_hello(&self.config.random, sni, out)
    }

    /// Feeds ServerHello record.
    pub fn handle_server_hello(&mut self, record: &[u8]) -> Result<(), TlsError> {
        self.require_live_identity()?;
        if record.len() < RECORD_HEADER_LEN {
            return Err(TlsError::BadRecordLength);
        }
        let payload = &record[RECORD_HEADER_LEN..];
        self.handshake.process_server_hello(payload)?;

        // Initialize handshake traffic ciphers
        self.send_cipher = Some(RecordCipher::new(
            self.handshake.client_handshake_key,
            self.handshake.client_handshake_iv,
        ));
        self.recv_cipher = Some(RecordCipher::new(
            self.handshake.server_handshake_key,
            self.handshake.server_handshake_iv,
        ));

        Ok(())
    }

    /// Opens one protected handshake record into `out` and reports how many
    /// plaintext bytes arrived.
    ///
    /// WHY the caller brings the buffer: the room a handshake record needs is
    /// the room the server's certificate chain takes, and a public chain is
    /// bigger than any frame this client used to hold. Measured on the wire, the
    /// answer to `https://developer.mozilla.org/` seals 4130 bytes into a single
    /// record, so the buffer has to be chosen where the record is read rather
    /// than in here.
    pub fn open_handshake_record(
        &mut self,
        record: &[u8],
        out: &mut [u8],
    ) -> Result<usize, TlsError> {
        let recv_cipher = self.recv_cipher.as_mut().ok_or(TlsError::UnexpectedMessage)?;
        let (content_type, payload_len) = recv_cipher.decrypt_record(record, out)?;

        if content_type != ContentType::Handshake {
            return Err(TlsError::UnexpectedMessage);
        }
        if payload_len == 0 {
            return Err(TlsError::BadHandshakeMessage);
        }
        Ok(payload_len)
    }

    /// Applies one complete handshake message.
    ///
    /// WHY a whole message rather than a record: the server decides where its
    /// records end, and TLS lets it write several handshake messages into one
    /// record and split one message across several. Treating the first byte of a
    /// record as a message type is only right when the record boundary happens
    /// to be a message boundary, so the boundaries are rejoined by
    /// [`HandshakeReader`] before this function is called.
    pub fn handle_handshake_message(&mut self, message: &[u8]) -> Result<(), TlsError> {
        let identity = self.require_live_identity()?;
        let verifier: &'a dyn CertificateVerifier = self.config.verifier;
        let Some(&message_type) = message.first() else {
            return Err(TlsError::BadHandshakeMessage);
        };

        match message_type {
            HANDSHAKE_ENCRYPTED_EXTENSIONS => {
                self.handshake.process_encrypted_extensions(message)?;
            }
            HANDSHAKE_CERTIFICATE => {
                let cert = self.handshake.process_certificate(message, verifier, identity)?;
                // Pin the verified leaf key for CertificateVerify. A key larger
                // than the pin can never be pinned whole, so the handshake stops
                // here instead of carrying a truncated identity forward — a
                // clipped RSA modulus would still verify nothing, but it would
                // look like a key the client had checked.
                let key = cert.spki_public_key;
                if key.is_empty() || key.len() > self.server_cert_spki.len() {
                    self.has_server_cert = false;
                    self.server_cert_spki_len = 0;
                    return Err(TlsError::CertificateVerificationFailed);
                }
                self.server_cert_spki[..key.len()].copy_from_slice(key);
                self.server_cert_spki_len = key.len();
                self.has_server_cert = true;
            }
            HANDSHAKE_CERTIFICATE_VERIFY => {
                // The signature is only ever checked against the leaf key the
                // client validated in the Certificate step. There is no
                // substitute key and no synthetic certificate on this path.
                if !self.has_server_cert {
                    return Err(TlsError::UnexpectedMessage);
                }
                let len = self.server_cert_spki_len;
                self.handshake
                    .process_certificate_verify(message, &self.server_cert_spki[..len], verifier)?;
            }
            HANDSHAKE_FINISHED => {
                self.handshake.process_server_finished(message)?;

                // Initialize application traffic ciphers
                self.app_send_cipher = Some(RecordCipher::new(
                    self.handshake.client_application_key,
                    self.handshake.client_application_iv,
                ));
                self.app_recv_cipher = Some(RecordCipher::new(
                    self.handshake.server_application_key,
                    self.handshake.server_application_iv,
                ));
            }
            _ => return Err(TlsError::UnexpectedMessage),
        }

        Ok(())
    }

    /// Emits Client Finished record into `out`.
    pub fn emit_client_finished(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        let send_cipher = self.send_cipher.as_mut().ok_or(TlsError::UnexpectedMessage)?;
        self.handshake.encode_client_finished(send_cipher, out)
    }

    /// Encrypts application data into `out`.
    pub fn encrypt_application_data(&mut self, plaintext: &[u8], out: &mut [u8]) -> Result<usize, TlsError> {
        let app_cipher = self.app_send_cipher.as_mut().ok_or(TlsError::UnexpectedMessage)?;
        app_cipher.encrypt_record(ContentType::ApplicationData, plaintext, out)
    }

    /// Verifies one record and unmasks it in the buffer that holds it, reporting
    /// the protected inner type -- after the handshake the record layer carries
    /// more than the document.
    pub fn open_record_in_place(
        &mut self,
        buf: &mut [u8],
    ) -> Result<(ContentType, usize), TlsError> {
        let app_cipher = self.app_recv_cipher.as_mut().ok_or(TlsError::UnexpectedMessage)?;
        app_cipher.open_record_in_place(buf)
    }

    pub fn is_connected(&self) -> bool {
        self.handshake.state == HandshakeState::Connected
    }
}

/// TLS 1.3 Client wrapping an underlying `TlsStream`.
pub struct TlsClient<'a, S: TlsStream> {
    stream: S,
    pub session: TlsClientSession<'a>,
    /// One record as it arrived, unmasked in place once its tag has verified.
    record: [u8; MAX_READ_RECORD_BYTES],
    /// End of the authenticated payload inside `record`, counted from the byte
    /// after the header. Zero until a record has been opened.
    payload_end: usize,
    /// How much of that payload the caller has already taken.
    payload_pos: usize,
}

impl<'a, S: TlsStream> TlsClient<'a, S> {
    /// Creates and connects a TLS 1.3 client over the provided stream.
    pub fn new(stream: S, config: TlsConfig<'a>) -> Self {
        let session = TlsClientSession::new(config);
        Self {
            stream,
            session,
            record: [0u8; MAX_READ_RECORD_BYTES],
            payload_end: 0,
            payload_pos: 0,
        }
    }

    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub fn stream(&self) -> &S {
        &self.stream
    }

    /// Sends application data over the encrypted TLS connection.
    pub fn write(&mut self, plaintext: &[u8]) -> Result<usize, TlsError> {
        if !self.session.is_connected() {
            return Err(TlsError::UnexpectedMessage);
        }
        let mut record_buf = [0u8; MAX_RECORD_BYTES];
        let record_len = self.session.encrypt_application_data(plaintext, &mut record_buf)?;
        self.stream.write_all(&record_buf[..record_len])?;
        Ok(plaintext.len())
    }

    /// Gathers exactly one record into the buffer this client owns and reports
    /// how many bytes it occupies, header included. `Ok(0)` means the peer
    /// stopped talking before a header arrived.
    ///
    /// WHY the whole record: a server fills a record to the protocol limit, and
    /// the limit is above what the device's TCP receive answers, so the gather
    /// loops over bounded requests rather than refusing the record.
    fn read_record_into(&mut self) -> Result<usize, TlsError> {
        let mut read_bytes = 0;
        while read_bytes < RECORD_HEADER_LEN {
            let n = self
                .stream
                .read(&mut self.record[read_bytes..RECORD_HEADER_LEN])?;
            if n == 0 {
                return Ok(0);
            }
            read_bytes += n;
        }

        let record_len = u16::from_be_bytes([self.record[3], self.record[4]]) as usize;
        let total_len = RECORD_HEADER_LEN + record_len;
        if total_len > self.record.len() {
            return Err(TlsError::RecordTooLarge);
        }

        while read_bytes < total_len {
            let want = (total_len - read_bytes).min(TRANSPORT_READ_BYTES);
            let n = self.stream.read(&mut self.record[read_bytes..read_bytes + want])?;
            if n == 0 {
                return Err(TlsError::BadRecordLength);
            }
            read_bytes += n;
        }
        Ok(total_len)
    }

    /// Reads records until the ServerHello, and hands it to the session.
    ///
    /// WHY the compatibility record is stepped over: RFC 8446 5.3 has a server
    /// that got a middlebox-compatible session id send a plaintext
    /// ChangeCipherSpec before its real first record, and every public server
    /// measured here does.
    pub fn read_server_hello(&mut self) -> Result<(), TlsError> {
        let mut skipped = 0usize;
        loop {
            let total_len = self.read_record_into()?;
            if total_len == 0 {
                return Err(TlsError::BadRecordLength);
            }
            if self.record[0] == ContentType::ChangeCipherSpec as u8 {
                skipped += 1;
                if skipped >= MAX_SKIPPED_RECORDS {
                    return Err(TlsError::UnexpectedMessage);
                }
                continue;
            }
            return self.session.handle_server_hello(&self.record[..total_len]);
        }
    }

    /// Opens the next handshake record into `out` and reports how many plaintext
    /// bytes arrived, stepping over the plaintext ChangeCipherSpec a server may
    /// still send after the handshake is encrypted.
    ///
    /// WHY `out` is the caller's: the bytes belong to a [`HandshakeReader`],
    /// which is the only thing that can tell where one message ends and the next
    /// begins, so the payload goes to the buffer that keeps the messages.
    pub fn read_handshake_payload(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        let mut skipped = 0usize;
        loop {
            let total_len = self.read_record_into()?;
            if total_len == 0 {
                return Ok(0);
            }
            if self.record[0] == ContentType::ChangeCipherSpec as u8 {
                skipped += 1;
                if skipped >= MAX_SKIPPED_RECORDS {
                    return Err(TlsError::UnexpectedMessage);
                }
                continue;
            }
            return self
                .session
                .open_handshake_record(&self.record[..total_len], out);
        }
    }

    /// Reads the next application data, filling the caller's window from a
    /// record of any size the protocol allows.
    ///
    /// WHY a record that is not application data is stepped over rather than
    /// taken as a failure: RFC 8446 4.6.1 has the server send one or more
    /// NewSessionTicket records as soon as the handshake completes, so the first
    /// record a GET meets on a real server is a ticket, not the page. Reading it
    /// as an error ends the fetch with zero bytes and the screen then blames the
    /// gateway for an answer it did send. Tickets are dropped here because this
    /// client never resumes a session.
    ///
    /// WHY the skip is bounded and the rest still refused: a peer can stream
    /// records that are not the document forever, and an alert genuinely means
    /// the peer stopped talking. Both have to end with something the reader can
    /// name, not with a silent empty read.
    ///
    /// WHY the answer is handed over in pieces: a server puts the opening
    /// kilobytes of a page inside one record of up to 16 KiB, while the caller
    /// reads through a window of its own choosing. The record is authenticated
    /// whole before any of it leaves this function, and then fills the caller's
    /// window as many times as that window is smaller than the record.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        if !self.session.is_connected() {
            return Err(TlsError::UnexpectedMessage);
        }

        if self.payload_pos < self.payload_end {
            return self.take_payload(out);
        }

        let mut skipped = 0usize;
        loop {
            let total_len = self.read_record_into()?;
            if total_len == 0 {
                return Ok(0); // EOF
            }

            let (content_type, len) = self
                .session
                .open_record_in_place(&mut self.record[..total_len])?;
            // WHY an empty application-data record is stepped over: `Ok(0)` is
            // this reader's only word for the peer having stopped talking, and a
            // record carrying no bytes is the peer still talking.
            match content_type {
                ContentType::ApplicationData if len > 0 => {
                    self.payload_end = len;
                    self.payload_pos = 0;
                    return self.take_payload(out);
                }
                ContentType::ApplicationData => {}
                ContentType::Handshake
                    if is_new_session_ticket(
                        &self.record[RECORD_HEADER_LEN..RECORD_HEADER_LEN + len],
                    ) => {}
                ContentType::ChangeCipherSpec => {}
                _ => return Err(TlsError::UnexpectedMessage),
            }
            skipped += 1;
            if skipped >= MAX_SKIPPED_RECORDS {
                return Err(TlsError::UnexpectedMessage);
            }
        }
    }

    /// Copies the next piece of the record this client is still holding.
    ///
    /// WHY the payload stays behind the header: sliding it to the front of the
    /// buffer would copy the whole record, up to 16 KiB, to make one index
    /// smaller, and the bytes are about to be copied into the caller's window
    /// anyway.
    fn take_payload(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        let start = RECORD_HEADER_LEN + self.payload_pos;
        let available = self.payload_end - self.payload_pos;
        let n = available.min(out.len());
        out[..n].copy_from_slice(&self.record[start..start + n]);
        self.payload_pos += n;
        Ok(n)
    }
}

/// Does this decrypted payload hold a `NewSessionTicket` handshake message?
///
/// WHY only the one type is ignorable: the message type is the first byte of the
/// payload, so a ticket is distinguishable from any other post-handshake message
/// without parsing it. A KeyUpdate or an unexpected Certificate would fall
/// through to the refusal, because continuing to read with the old keys after a
/// key change would decrypt garbage and present it as the peer's answer.
fn is_new_session_ticket(payload: &[u8]) -> bool {
    payload.first() == Some(&HANDSHAKE_NEW_SESSION_TICKET)
}
