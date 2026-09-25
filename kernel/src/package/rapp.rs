//! Canonical RAPP v2 binary package parser and verifier.
//!
//! RAPP v2 is the kernel execution format. ZIP/JSON bundles are tooling-only.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::package::revocation::REVOCATION_REGISTRY;

pub const RAPP_MAGIC: &[u8; 4] = b"RAPP";
pub const RAPP_FORMAT_VERSION: u16 = 2;
pub const RAPP_HEADER_SIZE: usize = 0xB4;
pub const RAPP_SIGNATURE_OFFSET: usize = 0x70;
pub const RAPP_SIGNATURE_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RappArch { X86_64=0, Aarch64=1, Riscv64=2 }
impl RappArch { pub fn from_u16(v:u16)->Option<Self>{match v{0=>Some(Self::X86_64),1=>Some(Self::Aarch64),2=>Some(Self::Riscv64),_=>None}} }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RappError {
    BufferTooShort, InvalidMagic, UnsupportedVersion, UnsupportedArch,
    InvalidPackageId, InvalidPublisherKey, DigestMismatch, SignatureInvalid,
    PublisherUntrusted, PublisherRevoked, PayloadOverflow, UnknownCapability,
    TooManyCapabilities, InvalidPayload,
}

#[derive(Debug, Clone)]
pub struct RappManifest {
    pub package_id:String,
    pub publisher_key:[u8;32],
    pub version:(u16,u16,u16),
    pub arch:RappArch,
    pub capabilities:Vec<u16>,
    pub payload_digest:[u8;SHA256_DIGEST_LENGTH],
    pub signature:[u8;64],
    pub payload_offset:usize,
    pub payload_len:usize,
}

fn fixed_utf8(data:&[u8])->Result<String,RappError>{
    let end=data.iter().position(|&b|b==0).unwrap_or(data.len());
    core::str::from_utf8(&data[..end]).map(|s|s.to_string()).map_err(|_|RappError::InvalidPackageId)
}

fn signed_message(data:&[u8], payload_offset:usize, payload_len:usize)->Vec<u8>{
    let mut msg=Vec::with_capacity(0xA4 + data.len().saturating_sub(payload_offset));
    msg.extend_from_slice(&data[..RAPP_SIGNATURE_OFFSET]);
    msg.extend_from_slice(&[0u8;RAPP_SIGNATURE_SIZE]);
    msg.extend_from_slice(&data[RAPP_SIGNATURE_OFFSET+RAPP_SIGNATURE_SIZE..payload_offset]);
    msg.extend_from_slice(&data[payload_offset..payload_offset+payload_len]);
    msg
}

pub fn verify_rapp_signature(data:&[u8], publisher_key:&[u8;32], signature:&[u8;64], payload_offset:usize, payload_len:usize)->Result<(),RappError>{
    let key=VerifyingKey::from_bytes(publisher_key).map_err(|_|RappError::InvalidPublisherKey)?;
    let sig=Signature::from_bytes(signature);
    let msg=signed_message(data,payload_offset,payload_len);
    key.verify(&msg,&sig).map_err(|_|RappError::SignatureInvalid)
}

pub fn parse_rapp(data:&[u8])->Result<RappManifest,RappError>{
    if data.len()<RAPP_HEADER_SIZE{return Err(RappError::BufferTooShort)}
    if &data[..4]!=RAPP_MAGIC{return Err(RappError::InvalidMagic)}
    if u16::from_le_bytes([data[4],data[5]])!=RAPP_FORMAT_VERSION{return Err(RappError::UnsupportedVersion)}
    let arch=RappArch::from_u16(u16::from_le_bytes([data[6],data[7]])).ok_or(RappError::UnsupportedArch)?;
    let package_id=fixed_utf8(&data[8..40])?;
    if package_id.is_empty(){return Err(RappError::InvalidPackageId)}
    let mut publisher_key=[0u8;32]; publisher_key.copy_from_slice(&data[40..72]);
    if publisher_key.iter().all(|&b|b==0){return Err(RappError::InvalidPublisherKey)}
    let version=(u16::from_le_bytes([data[72],data[73]]),u16::from_le_bytes([data[74],data[75]]),u16::from_le_bytes([data[76],data[77]]));
    let cap_count=u16::from_le_bytes([data[78],data[79]]) as usize;
    if cap_count>1024{return Err(RappError::TooManyCapabilities)}
    let mut digest=[0u8;32]; digest.copy_from_slice(&data[0x50..0x70]);
    let mut signature=[0u8;64]; signature.copy_from_slice(&data[0x70..0xB0]);
    // v2 keeps the documented compact header offsets but the key/signature expansion
    // requires the actual capability/payload base to move to 0xB4.
    let payload_len=u32::from_le_bytes([data[0xB0],data[0xB1],data[0xB2],data[0xB3]]) as usize;
    let header_end=RAPP_HEADER_SIZE;
    let caps_end=header_end.checked_add(cap_count.checked_mul(2).ok_or(RappError::PayloadOverflow)?).ok_or(RappError::PayloadOverflow)?;
    if data.len()<caps_end{return Err(RappError::BufferTooShort)}
    let mut capabilities=Vec::with_capacity(cap_count);
    for i in 0..cap_count{let o=header_end+i*2;let cap=u16::from_le_bytes([data[o],data[o+1]]);
        if !(1..=18).contains(&cap){return Err(RappError::UnknownCapability)}
        capabilities.push(cap);}
    let payload_offset=caps_end;
    let payload_end=payload_offset.checked_add(payload_len).ok_or(RappError::PayloadOverflow)?;
    if data.len()<payload_end{return Err(RappError::PayloadOverflow)}
    if data.len()!=payload_end{return Err(RappError::PayloadOverflow)}
    let computed=Sha256::digest(&data[payload_offset..payload_end]);
    if computed!=digest{return Err(RappError::DigestMismatch)}
    let reg=REVOCATION_REGISTRY.lock();
    if reg.is_key_revoked(&publisher_key){return Err(RappError::PublisherRevoked)}
    if !reg.is_key_trusted(&publisher_key){return Err(RappError::PublisherUntrusted)}
    drop(reg);
    verify_rapp_signature(data,&publisher_key,&signature,payload_offset,payload_len)?;
    Ok(RappManifest{package_id,publisher_key,version,arch,capabilities,payload_digest:digest,signature,payload_offset,payload_len})
}

/// Encodes a RAPP v2 package. The caller must supply a real Ed25519 signature
/// over the canonical message; zero signatures are intentionally rejected by parse_rapp.
pub fn build_rapp(package_id:&str,publisher_key:&[u8;32],version:(u16,u16,u16),arch:RappArch,capabilities:&[u16],elf_payload:&[u8],signature:&[u8;64])->Vec<u8>{
    let digest=Sha256::digest(elf_payload); let header_end=RAPP_HEADER_SIZE;
    let mut b=Vec::with_capacity(header_end+capabilities.len()*2+elf_payload.len());
    b.extend_from_slice(RAPP_MAGIC); b.extend_from_slice(&RAPP_FORMAT_VERSION.to_le_bytes()); b.extend_from_slice(&(arch as u16).to_le_bytes());
    let mut pkg=[0u8;32]; let n=package_id.as_bytes().len().min(32); pkg[..n].copy_from_slice(&package_id.as_bytes()[..n]); b.extend_from_slice(&pkg);
    b.extend_from_slice(publisher_key); b.extend_from_slice(&version.0.to_le_bytes()); b.extend_from_slice(&version.1.to_le_bytes()); b.extend_from_slice(&version.2.to_le_bytes()); b.extend_from_slice(&(capabilities.len() as u16).to_le_bytes());
    b.extend_from_slice(&digest); b.extend_from_slice(signature); b.extend_from_slice(&(elf_payload.len() as u32).to_le_bytes());
    for &cap in capabilities{b.extend_from_slice(&cap.to_le_bytes())} b.extend_from_slice(elf_payload); b
}
