// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The TLS trust anchors installed on this device, read from the kernel.
//!
//! A chain verifies only against an anchor the operator put on this device. The
//! OS ships none, so a device that never had one installed verifies nothing --
//! this module reads whatever is there, and the caller says honestly what it
//! found. See the kernel's `crypto::tls_trust` for who may install an anchor and
//! where the store lives.
//!
//! WHY the table is `static mut` rather than a value the caller owns:
//! [`crate::tls::DefaultCertVerifier::with_roots`] takes `&'a [&'a [u8]]`, and the
//! verifier a client builds has to outlive the handshake that consumes it. A
//! self-referential struct cannot be returned by value, so the anchor bytes and
//! the borrowed views of them sit in static storage the browser process owns for
//! its whole life -- the same shape `image.rs` uses for its decode scratch buffers.
//!
//! WHY a reload writes the whole table before handing out the views: the views
//! this module returns stay valid across a reload because the buffers they point
//! to are never moved. Reloading replaces the bytes in place, so a client that
//! kept an older view reads the new anchor instead of the one it asked for. The
//! browser therefore reloads immediately before it builds a verifier and holds no
//! view across a reload.

use crate::{tls_trust_count, tls_trust_read, TLS_TRUST_ANCHOR_BYTES, TLS_TRUST_MAX_ANCHORS};

/// One anchor as the kernel handed it over, with the bytes past its end ignored.
#[derive(Clone, Copy)]
struct Slot {
    der: [u8; TLS_TRUST_ANCHOR_BYTES],
    len: usize,
}

const EMPTY_SLOT: Slot = Slot {
    der: [0u8; TLS_TRUST_ANCHOR_BYTES],
    len: 0,
};

const EMPTY_REF: &[u8] = &[];

static mut ANCHOR_SLOTS: [Slot; TLS_TRUST_MAX_ANCHORS] = [EMPTY_SLOT; TLS_TRUST_MAX_ANCHORS];
static mut ANCHOR_VIEWS: [&'static [u8]; TLS_TRUST_MAX_ANCHORS] = [EMPTY_REF; TLS_TRUST_MAX_ANCHORS];

/// Why the store could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The kernel would not answer. Either this process was not granted
    /// `tls.trust.read`, or the image it runs on predates the trust store. The
    /// anchors on the device are unknown, which is not the same as there being
    /// none, so a client must not report an empty store here.
    Refused,
    /// Kernel and userspace disagree about how big the store is: the kernel
    /// named more anchors than this table has slots, or answered with more bytes
    /// than a slot can hold. Reading on would verify against part of the store
    /// and present the missing anchors as if they were absent.
    SizeMismatch,
}

impl ReadError {
    pub const fn message(self) -> &'static str {
        match self {
            ReadError::Refused => "the kernel refused to read the device trust store",
            ReadError::SizeMismatch => "the device trust store is larger than this client can hold",
        }
    }
}

/// Reads every anchor the device holds and returns the table a verifier consumes.
///
/// An empty return value is a real answer: the operator has installed nothing, so
/// no chain can verify on this device. A caller that renders a page should say
/// that in those words rather than blaming the peer's certificate.
///
/// WHY the whole store is read on every call: the operator can install or revoke
/// an anchor from the console at any moment, and a client that cached the store at
/// startup would keep refusing a chain it could verify after that line was typed.
pub fn load() -> Result<&'static [&'static [u8]], ReadError> {
    let count = tls_trust_count().map_err(|_| ReadError::Refused)?;
    if count > TLS_TRUST_MAX_ANCHORS {
        return Err(ReadError::SizeMismatch);
    }

    let slots = unsafe { &mut *core::ptr::addr_of_mut!(ANCHOR_SLOTS) };
    let views = unsafe { &mut *core::ptr::addr_of_mut!(ANCHOR_VIEWS) };

    for position in 0..count {
        let slot = &mut slots[position];
        slot.len = 0;
        let read = tls_trust_read(position, &mut slot.der).map_err(|_| ReadError::Refused)?;
        if read > TLS_TRUST_ANCHOR_BYTES {
            return Err(ReadError::SizeMismatch);
        }
        slot.len = read;
        // The bytes live in static storage that this process never moves, so the
        // view outlives every handshake it is used for. See the module comment.
        views[position] = unsafe { core::slice::from_raw_parts(slot.der.as_ptr(), read) };
    }
    // Slots past the end read as no anchor at all, so a store that shrank since
    // the last load cannot leave an old view behind for a client to trust.
    for position in count..TLS_TRUST_MAX_ANCHORS {
        views[position] = EMPTY_REF;
        slots[position].len = 0;
    }

    let loaded = unsafe { &*core::ptr::addr_of!(ANCHOR_VIEWS) };
    Ok(&loaded[..count])
}
