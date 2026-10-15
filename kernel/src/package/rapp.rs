//! Canonical RAPP v2 binary package parser and verifier.
//!
//! RAPP v2 is the kernel execution format. ZIP/JSON bundles are tooling-only.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ed25519_dalek::{Signature, VerifyingKey};
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::package::revocation::REVOCATION_REGISTRY;
use crate::package::dependency::{Dependency, PackagePolicy, PackageVersion, VersionRequirement};

pub const RAPP_MAGIC: &[u8; 4] = b"RAPP";
pub const RAPP_FORMAT_VERSION: u16 = 2;
pub const RAPP_FORMAT_VERSION_V3: u16 = 3;
pub const RAPP_HEADER_SIZE: usize = 0xB4;
pub const RAPP_V3_HEADER_SIZE: usize = 0xB8;
pub const RAPP_MAX_DEPENDENCIES: usize = 32;
pub const RAPP_MAX_PACKAGE_ID_LEN: usize = 32;
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
    PublisherUntrusted, PublisherRevoked, PackageRevoked, PayloadOverflow, UnknownCapability,
    TooManyCapabilities, InvalidPayload, InvalidDependency, TooManyDependencies,
}

#[derive(Debug, Clone)]
pub struct RappManifest {
    pub format_version:u16,
    pub package_id:String,
    pub publisher_key:[u8;32],
    pub version:(u16,u16,u16),
    pub arch:RappArch,
    pub capabilities:Vec<u16>,
    pub payload_digest:[u8;SHA256_DIGEST_LENGTH],
    pub signature:[u8;64],
    pub payload_offset:usize,
    pub payload_len:usize,
    pub policy: PackagePolicy,
    pub dependencies:Vec<Dependency>,
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
    key.verify_strict(&msg,&sig).map_err(|_|RappError::SignatureInvalid)
}

pub fn parse_rapp(data:&[u8])->Result<RappManifest,RappError>{
    if data.len()<RAPP_HEADER_SIZE{return Err(RappError::BufferTooShort)}
    if &data[..4]!=RAPP_MAGIC{return Err(RappError::InvalidMagic)}
    let format_version=u16::from_le_bytes([data[4],data[5]]);
    if format_version!=RAPP_FORMAT_VERSION && format_version!=RAPP_FORMAT_VERSION_V3{return Err(RappError::UnsupportedVersion)}
    let header_end=if format_version==RAPP_FORMAT_VERSION_V3{RAPP_V3_HEADER_SIZE}else{RAPP_HEADER_SIZE};
    if data.len()<header_end{return Err(RappError::BufferTooShort)}
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
    let payload_len=u32::from_le_bytes([data[0xB0],data[0xB1],data[0xB2],data[0xB3]]) as usize;
    let dependency_len=if format_version==RAPP_FORMAT_VERSION_V3 {
        u32::from_le_bytes([data[0xB4],data[0xB5],data[0xB6],data[0xB7]]) as usize
    } else { 0 };
    let caps_end=header_end.checked_add(cap_count.checked_mul(2).ok_or(RappError::PayloadOverflow)?).ok_or(RappError::PayloadOverflow)?;
    if data.len()<caps_end{return Err(RappError::BufferTooShort)}
    let mut capabilities=Vec::with_capacity(cap_count);
    for i in 0..cap_count{
        let o=header_end+i*2;
        let cap=u16::from_le_bytes([data[o],data[o+1]]);
        if crate::capability::CapabilityType::from_raw(cap as u64).is_none() {
            return Err(RappError::UnknownCapability);
        }
        capabilities.push(cap);
    }
    let dependency_start=caps_end;
    let dependency_end=dependency_start.checked_add(dependency_len).ok_or(RappError::PayloadOverflow)?;
    if data.len()<dependency_end{return Err(RappError::PayloadOverflow)}
    let mut dependencies=Vec::new();
    if format_version==RAPP_FORMAT_VERSION_V3 {
        if dependency_len<2{return Err(RappError::InvalidDependency)}
        let dep_count=u16::from_le_bytes([data[dependency_start],data[dependency_start+1]]) as usize;
        if dep_count>RAPP_MAX_DEPENDENCIES{return Err(RappError::TooManyDependencies)}
        let mut cursor=dependency_start+2;
        for _ in 0..dep_count {
            if cursor>=dependency_end{return Err(RappError::InvalidDependency)}
            let id_len=data[cursor] as usize; cursor+=1;
            if id_len==0 || id_len>RAPP_MAX_PACKAGE_ID_LEN || cursor.checked_add(id_len+7).ok_or(RappError::InvalidDependency)?>dependency_end {
                return Err(RappError::InvalidDependency);
            }
            let package_id=core::str::from_utf8(&data[cursor..cursor+id_len]).map_err(|_|RappError::InvalidDependency)?.to_string();
            cursor+=id_len;
            let kind=data[cursor]; cursor+=1;
            let major=u16::from_le_bytes([data[cursor],data[cursor+1]]);
            let minor=u16::from_le_bytes([data[cursor+2],data[cursor+3]]);
            let patch=u16::from_le_bytes([data[cursor+4],data[cursor+5]]);
            cursor+=6;
            let requirement=match kind {
                0=>VersionRequirement::Exact(PackageVersion::new(major,minor,patch)),
                1=>VersionRequirement::Minimum(PackageVersion::new(major,minor,patch)),
                2=>VersionRequirement::Major(major),
                _=>return Err(RappError::InvalidDependency),
            };
            dependencies.push(Dependency{package_id,requirement});
        }
        if cursor!=dependency_end{return Err(RappError::InvalidDependency)}
    } else if dependency_len!=0 {
        return Err(RappError::InvalidDependency);
    }
    let payload_offset=dependency_end;
    let payload_end=payload_offset.checked_add(payload_len).ok_or(RappError::PayloadOverflow)?;
    if data.len()<payload_end || data.len()!=payload_end{return Err(RappError::PayloadOverflow)}
    let computed=Sha256::digest(&data[payload_offset..payload_end]);
    if computed!=digest{return Err(RappError::DigestMismatch)}
    let reg=REVOCATION_REGISTRY.lock();
    if reg.is_package_revoked(&package_id){return Err(RappError::PackageRevoked)}
    if reg.is_key_revoked(&publisher_key){return Err(RappError::PublisherRevoked)}
    if !reg.is_key_trusted(&publisher_key){return Err(RappError::PublisherUntrusted)}
    drop(reg);
    verify_rapp_signature(data,&publisher_key,&signature,payload_offset,payload_len)?;
    let mut policy=PackagePolicy::from_rapp_capabilities(&capabilities);
    policy.dependencies=dependencies.clone();
    if !policy.validate(){return Err(RappError::InvalidDependency)}
    Ok(RappManifest{format_version,package_id,publisher_key,version,arch,policy,dependencies,capabilities,payload_digest:digest,signature,payload_offset,payload_len})
}


/// Encodes a RAPP v3 package with signed dependency metadata.
///
/// The dependency table is part of the signed message because the canonical
/// signature covers every byte between the signature field and the payload.
pub fn build_rapp_v3(
    package_id:&str,
    publisher_key:&[u8;32],
    version:(u16,u16,u16),
    arch:RappArch,
    capabilities:&[u16],
    dependencies:&[Dependency],
    elf_payload:&[u8],
    signature:&[u8;64],
)->Vec<u8>{
    let digest=Sha256::digest(elf_payload);
    let mut dep=Vec::new();
    dep.extend_from_slice(&(dependencies.len() as u16).to_le_bytes());
    for d in dependencies {
        let id=d.package_id.as_bytes();
        let n=id.len().min(RAPP_MAX_PACKAGE_ID_LEN);
        dep.push(n as u8);
        dep.extend_from_slice(&id[..n]);
        match d.requirement {
            VersionRequirement::Exact(v)=>{dep.push(0);dep.extend_from_slice(&v.major.to_le_bytes());dep.extend_from_slice(&v.minor.to_le_bytes());dep.extend_from_slice(&v.patch.to_le_bytes());}
            VersionRequirement::Minimum(v)=>{dep.push(1);dep.extend_from_slice(&v.major.to_le_bytes());dep.extend_from_slice(&v.minor.to_le_bytes());dep.extend_from_slice(&v.patch.to_le_bytes());}
            VersionRequirement::Major(v)=>{dep.push(2);dep.extend_from_slice(&v.to_le_bytes());dep.extend_from_slice(&0u16.to_le_bytes());dep.extend_from_slice(&0u16.to_le_bytes());}
        }
    }
    let header_end=RAPP_V3_HEADER_SIZE;
    let mut b=Vec::with_capacity(header_end+capabilities.len()*2+dep.len()+elf_payload.len());
    b.extend_from_slice(RAPP_MAGIC); b.extend_from_slice(&RAPP_FORMAT_VERSION_V3.to_le_bytes()); b.extend_from_slice(&(arch as u16).to_le_bytes());
    let mut pkg=[0u8;32]; let n=package_id.as_bytes().len().min(32); pkg[..n].copy_from_slice(&package_id.as_bytes()[..n]); b.extend_from_slice(&pkg);
    b.extend_from_slice(publisher_key); b.extend_from_slice(&version.0.to_le_bytes()); b.extend_from_slice(&version.1.to_le_bytes()); b.extend_from_slice(&version.2.to_le_bytes()); b.extend_from_slice(&(capabilities.len() as u16).to_le_bytes());
    b.extend_from_slice(&digest); b.extend_from_slice(signature); b.extend_from_slice(&(elf_payload.len() as u32).to_le_bytes()); b.extend_from_slice(&(dep.len() as u32).to_le_bytes());
    for &cap in capabilities{b.extend_from_slice(&cap.to_le_bytes())}
    b.extend_from_slice(&dep); b.extend_from_slice(elf_payload); b
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use crate::package::revocation::{REVOCATION_REGISTRY, RevocationReason};

    fn create_signed_test_package(caps: &[u16], seed: u8) -> (Vec<u8>, [u8; 32], SigningKey) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let pub_key = signing_key.verifying_key().to_bytes();
        let payload = b"\x7fELF_USER_BINARY_SAMPLE";
        let placeholder_sig = [0u8; 64];

        let mut pkg = build_rapp(
            "org.interos.clock",
            &pub_key,
            (1, 0, 0),
            RappArch::X86_64,
            caps,
            payload,
            &placeholder_sig,
        );

        let payload_offset = RAPP_HEADER_SIZE + caps.len() * 2;
        let msg = signed_message(&pkg, payload_offset, payload.len());
        let sig = signing_key.sign(&msg);
        pkg[RAPP_SIGNATURE_OFFSET..RAPP_SIGNATURE_OFFSET + RAPP_SIGNATURE_SIZE]
            .copy_from_slice(&sig.to_bytes());

        (pkg, pub_key, signing_key)
    }

    #[test]
    fn parse_rapp_accepts_realtime_clock_read_capability() {
        let (pkg, pub_key, _) = create_signed_test_package(&[1, 24], 10);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "clock_test").unwrap();

        let manifest = parse_rapp(&pkg).expect("package requesting RealtimeClockRead (24) must parse");
        assert_eq!(manifest.package_id, "org.interos.clock");
        assert_eq!(manifest.capabilities, alloc::vec![1, 24]);
    }

    #[test]
    fn parse_rapp_rejects_unknown_capability_id() {
        let (pkg, pub_key, _) = create_signed_test_package(&[999], 11);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "unknown_cap_test").unwrap();

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::UnknownCapability);
    }

    #[test]
    fn parse_rapp_rejects_untrusted_publisher() {
        let (pkg, _pub_key, _) = create_signed_test_package(&[1], 12);
        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::PublisherUntrusted);
    }

    #[test]
    fn parse_rapp_rejects_revoked_key() {
        let (pkg, pub_key, _) = create_signed_test_package(&[1], 13);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "revoked_pub").unwrap();
        REVOCATION_REGISTRY.lock().place_key_on_crl(&pub_key, RevocationReason::KeyCompromised, 1, "root").unwrap();

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::PublisherRevoked);
    }
}
