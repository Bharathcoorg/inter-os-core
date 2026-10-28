//! Canonical RAPP v2 binary package parser and verifier.
//!
//! RAPP v2 is the kernel execution format. ZIP/JSON bundles are tooling-only.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ed25519_dalek::{Signature, VerifyingKey};
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::package::revocation::REVOCATION_REGISTRY;
use crate::package::dependency::{Dependency, PackagePolicy, PackageVersion, VersionRequirement};
use crate::content::intent::{IntentDeclaration, IntentKind};

pub const RAPP_MAGIC: &[u8; 4] = b"RAPP";
pub const RAPP_FORMAT_VERSION: u16 = 2;
pub const RAPP_FORMAT_VERSION_V3: u16 = 3;
pub const RAPP_FORMAT_VERSION_V4: u16 = 4;
pub const RAPP_HEADER_SIZE: usize = 0xB4;
pub const RAPP_V3_HEADER_SIZE: usize = 0xB8;
pub const RAPP_MAX_DEPENDENCIES: usize = 32;
pub const RAPP_MAX_PACKAGE_ID_LEN: usize = 32;
pub const RAPP_SIGNATURE_OFFSET: usize = 0x70;
pub const RAPP_SIGNATURE_SIZE: usize = 64;
/// The largest number of intent declarations one package may carry.
pub const RAPP_MAX_INTENTS: usize = 64;
/// The largest length of one intent's value.
pub const RAPP_MAX_INTENT_VALUE_LEN: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    Trusted,
    Untrusted,
    Revoked,
    Tampered,
}

impl TrustState {
    pub const fn as_u8(&self) -> u8 {
        match *self {
            TrustState::Trusted => 0,
            TrustState::Untrusted => 1,
            TrustState::Revoked => 2,
            TrustState::Tampered => 3,
        }
    }

    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(TrustState::Trusted),
            1 => Some(TrustState::Untrusted),
            2 => Some(TrustState::Revoked),
            3 => Some(TrustState::Tampered),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RappArch { X86_64=0, Aarch64=1, Riscv64=2, Wasm=3 }
impl RappArch { pub fn from_u16(v:u16)->Option<Self>{match v{0=>Some(Self::X86_64),1=>Some(Self::Aarch64),2=>Some(Self::Riscv64),3=>Some(Self::Wasm),_=>None}} }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RappError {
    BufferTooShort, InvalidMagic, UnsupportedVersion, UnsupportedArch,
    InvalidPackageId, InvalidPublisherKey, DigestMismatch, SignatureInvalid,
    PublisherUntrusted, PublisherRevoked, PackageRevoked, PayloadOverflow, UnknownCapability,
    TooManyCapabilities, InvalidPayload, InvalidDependency, TooManyDependencies, TooManyIntents,
    InvalidIntent,
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
    /// What this package declares it can handle.
    ///
    /// WHY the declarations travel in the signed manifest: "this package
    /// handles image/png" is a claim the device acts on, and a claim that
    /// could be edited after signing would be no claim at all.
    pub intents: Vec<IntentDeclaration>,
    /// The discrete cryptographic trust state of this container.
    pub trust_state: TrustState,
}

fn fixed_utf8(data:&[u8])->Result<String,RappError>{
    let end=data.iter().position(|&b|b==0).unwrap_or(data.len());
    core::str::from_utf8(&data[..end]).map(|s|s.to_string()).map_err(|_|RappError::InvalidPackageId)
}

pub(crate) fn signed_message(data:&[u8], payload_offset:usize, payload_len:usize)->Vec<u8>{
    let mut msg=Vec::with_capacity(0xA4 + data.len().saturating_sub(payload_offset));
    msg.extend_from_slice(&data[..RAPP_SIGNATURE_OFFSET]);
    msg.extend_from_slice(&[0u8;RAPP_SIGNATURE_SIZE]);
    msg.extend_from_slice(&data[RAPP_SIGNATURE_OFFSET+RAPP_SIGNATURE_SIZE..payload_offset]);
    msg.extend_from_slice(&data[payload_offset..payload_offset+payload_len]);
    msg
}

/// Builds a genuinely signed RAPP for tests.
///
/// WHY this is shared rather than duplicated per test module: a test that
/// verifies a package must present bytes `parse_rapp` actually accepts, and a
/// hand-rolled copy in each module drifts from the real signing rule.
#[cfg(test)]
pub(crate) fn build_signed_test_package(
    package_id: &str,
    capabilities: &[u16],
    payload: &[u8],
    seed: u8,
) -> (Vec<u8>, [u8; 32]) {
    use ed25519_dalek::{Signer, SigningKey};
    let signing_key = SigningKey::from_bytes(&[seed; 32]);
    let pub_key = signing_key.verifying_key().to_bytes();

    let mut pkg = build_rapp(
        package_id,
        &pub_key,
        (1, 0, 0),
        RappArch::X86_64,
        capabilities,
        payload,
        &[0u8; 64],
    );

    let payload_offset = RAPP_HEADER_SIZE + capabilities.len() * 2;
    let msg = signed_message(&pkg, payload_offset, payload.len());
    let sig = signing_key.sign(&msg);
    pkg[RAPP_SIGNATURE_OFFSET..RAPP_SIGNATURE_OFFSET + RAPP_SIGNATURE_SIZE]
        .copy_from_slice(&sig.to_bytes());

    (pkg, pub_key)
}

pub fn verify_rapp_signature(data:&[u8], publisher_key:&[u8;32], signature:&[u8;64], payload_offset:usize, payload_len:usize)->Result<(),RappError>{
    let key=VerifyingKey::from_bytes(publisher_key).map_err(|_|RappError::InvalidPublisherKey)?;
    let sig=Signature::from_bytes(signature);
    let msg=signed_message(data,payload_offset,payload_len);
    key.verify_strict(&msg,&sig).map_err(|_|RappError::SignatureInvalid)
}

/// Parses the RAPP v3 dependency table.
///
/// WHY the table is parsed from a slice and not from the whole package: the
/// v4 format carries the dependency table and the intent table in one signed
/// section, and both are read the same way.
fn parse_dependency_table(section:&[u8])->Result<Vec<Dependency>,RappError>{
    if section.len()<2{return Err(RappError::InvalidDependency)}
    let dep_count=u16::from_le_bytes([section[0],section[1]]) as usize;
    if dep_count>RAPP_MAX_DEPENDENCIES{return Err(RappError::TooManyDependencies)}
    let mut cursor=2usize;
    let mut dependencies=Vec::with_capacity(dep_count);
    for _ in 0..dep_count {
        if cursor>=section.len(){return Err(RappError::InvalidDependency)}
        let id_len=section[cursor] as usize; cursor+=1;
        if id_len==0 || id_len>RAPP_MAX_PACKAGE_ID_LEN || cursor.checked_add(id_len+7).ok_or(RappError::InvalidDependency)?>section.len() {
            return Err(RappError::InvalidDependency);
        }
        let package_id=core::str::from_utf8(&section[cursor..cursor+id_len]).map_err(|_|RappError::InvalidDependency)?.to_string();
        cursor+=id_len;
        let kind=section[cursor]; cursor+=1;
        let major=u16::from_le_bytes([section[cursor],section[cursor+1]]);
        let minor=u16::from_le_bytes([section[cursor+2],section[cursor+3]]);
        let patch=u16::from_le_bytes([section[cursor+4],section[cursor+5]]);
        cursor+=6;
        let requirement=match kind {
            0=>VersionRequirement::Exact(PackageVersion::new(major,minor,patch)),
            1=>VersionRequirement::Minimum(PackageVersion::new(major,minor,patch)),
            2=>VersionRequirement::Major(major),
            _=>return Err(RappError::InvalidDependency),
        };
        dependencies.push(Dependency{package_id,requirement});
    }
    if cursor!=section.len(){return Err(RappError::InvalidDependency)}
    Ok(dependencies)
}

/// Parses the RAPP v4 intent table.
///
/// WHY the kind is a byte and not a string: the table is inside a signed
/// binary structure, and a fixed-width kind keeps the parser total -- an
/// unknown kind is a refusal rather than a guess.
fn parse_intent_table(section:&[u8])->Result<Vec<IntentDeclaration>,RappError>{
    if section.len()<2{return Err(RappError::InvalidIntent)}
    let count=u16::from_le_bytes([section[0],section[1]]) as usize;
    if count>RAPP_MAX_INTENTS{return Err(RappError::TooManyIntents)}
    let mut cursor=2usize;
    let mut intents=Vec::with_capacity(count);
    for _ in 0..count {
        if cursor+2>section.len(){return Err(RappError::InvalidIntent)}
        let kind=IntentKind::from_raw(section[cursor] as u64).ok_or(RappError::InvalidIntent)?;
        cursor+=1;
        let value_len=section[cursor] as usize; cursor+=1;
        if value_len==0 || value_len>RAPP_MAX_INTENT_VALUE_LEN || cursor.checked_add(value_len).ok_or(RappError::InvalidIntent)?>section.len() {
            return Err(RappError::InvalidIntent);
        }
        let value=core::str::from_utf8(&section[cursor..cursor+value_len]).map_err(|_|RappError::InvalidIntent)?.to_string();
        cursor+=value_len;
        intents.push(IntentDeclaration{kind,value});
    }
    if cursor!=section.len(){return Err(RappError::InvalidIntent)}
    Ok(intents)
}

/// Parses the RAPP v4 signed section: a dependency table and an intent table.
fn parse_v4_section(section:&[u8])->Result<(Vec<Dependency>,Vec<IntentDeclaration>),RappError>{
    if section.len()<8{return Err(RappError::InvalidDependency)}
    let dep_len=u32::from_le_bytes([section[0],section[1],section[2],section[3]]) as usize;
    let intent_len=u32::from_le_bytes([section[4],section[5],section[6],section[7]]) as usize;
    let dep_end=8usize.checked_add(dep_len).ok_or(RappError::PayloadOverflow)?;
    let intent_end=dep_end.checked_add(intent_len).ok_or(RappError::PayloadOverflow)?;
    if intent_end!=section.len(){return Err(RappError::PayloadOverflow)}
    let dependencies=parse_dependency_table(&section[8..dep_end])?;
    let intents=parse_intent_table(&section[dep_end..intent_end])?;
    Ok((dependencies,intents))
}

pub fn parse_rapp(data:&[u8])->Result<RappManifest,RappError>{
    if data.len()<RAPP_HEADER_SIZE{return Err(RappError::BufferTooShort)}
    if &data[..4]!=RAPP_MAGIC{return Err(RappError::InvalidMagic)}
    let format_version=u16::from_le_bytes([data[4],data[5]]);
    if format_version!=RAPP_FORMAT_VERSION && format_version!=RAPP_FORMAT_VERSION_V3 && format_version!=RAPP_FORMAT_VERSION_V4{return Err(RappError::UnsupportedVersion)}
    let header_end=if format_version==RAPP_FORMAT_VERSION{RAPP_HEADER_SIZE}else{RAPP_V3_HEADER_SIZE};
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
    // WHY v2 has no section and v3's section is the dependency table: the
    // field at 0xB4 means "the length of the signed section that follows the
    // capability table", and in v3 that section happens to be only the
    // dependency table. In v4 it carries the intent table as well.
    let section_len=if format_version==RAPP_FORMAT_VERSION { 0 } else {
        u32::from_le_bytes([data[0xB4],data[0xB5],data[0xB6],data[0xB7]]) as usize
    };
    let caps_end=header_end.checked_add(cap_count.checked_mul(2).ok_or(RappError::PayloadOverflow)?).ok_or(RappError::PayloadOverflow)?;
    if data.len()<caps_end{return Err(RappError::BufferTooShort)}
    let mut capabilities=Vec::with_capacity(cap_count);
    for i in 0..cap_count{
        let o=header_end+i*2;
        let cap=u16::from_le_bytes([data[o],data[o+1]]);
        if !(1..=30).contains(&cap) {
            return Err(RappError::UnknownCapability);
        }
        capabilities.push(cap);
    }
    let section_start=caps_end;
    let section_end=section_start.checked_add(section_len).ok_or(RappError::PayloadOverflow)?;
    if data.len()<section_end{return Err(RappError::PayloadOverflow)}
    let (dependencies,intents)=match format_version {
        RAPP_FORMAT_VERSION_V4 => parse_v4_section(&data[section_start..section_end])?,
        RAPP_FORMAT_VERSION_V3 => (parse_dependency_table(&data[section_start..section_end])?, Vec::new()),
        _ => {
            if section_len!=0 { return Err(RappError::InvalidDependency); }
            (Vec::new(), Vec::new())
        }
    };
    let payload_offset=section_end;
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
    Ok(RappManifest{format_version,package_id,publisher_key,version,arch,policy,dependencies,intents,capabilities,payload_digest:digest,signature,payload_offset,payload_len,trust_state:TrustState::Trusted})
}

/// Inspects the discrete trust state of a `.rapp` container.
///
/// Returns a typed `TrustState` enum rather than a UI string:
/// - `TrustState::Trusted`: Valid structure, authentic Ed25519 signature, trusted publisher key, valid payload digest.
/// - `TrustState::Untrusted`: Container is structured properly but publisher key is not in the trust store.
/// - `TrustState::Revoked`: Package ID or publisher key is recorded on the CRL.
/// - `TrustState::Tampered`: Payload digest mismatch, invalid cryptographic signature, or corrupted container structure.
pub fn inspect_rapp_trust(data: &[u8]) -> TrustState {
    match parse_rapp(data) {
        Ok(manifest) => manifest.trust_state,
        Err(RappError::PublisherUntrusted) => TrustState::Untrusted,
        Err(RappError::PackageRevoked | RappError::PublisherRevoked) => TrustState::Revoked,
        Err(RappError::DigestMismatch | RappError::SignatureInvalid) => TrustState::Tampered,
        Err(_) => TrustState::Tampered,
    }
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
/// Encodes a RAPP v4 package with signed dependency and intent metadata.
///
/// WHY v4 exists rather than an extension of v3: the intent table is a
/// different kind of declaration from a dependency, and a package that
/// declares no intents must still be describable. v4 keeps the v3 header
/// layout, adds the intent table to the signed section, and leaves every
/// earlier format byte-for-byte unchanged.
pub fn build_rapp_v4(
    package_id:&str,
    publisher_key:&[u8;32],
    version:(u16,u16,u16),
    arch:RappArch,
    capabilities:&[u16],
    dependencies:&[Dependency],
    intents:&[IntentDeclaration],
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
    let mut intent=Vec::new();
    intent.extend_from_slice(&(intents.len() as u16).to_le_bytes());
    for i in intents {
        let value=i.value.as_bytes();
        let n=value.len().min(RAPP_MAX_INTENT_VALUE_LEN);
        intent.push(i.kind.as_raw() as u8);
        intent.push(n as u8);
        intent.extend_from_slice(&value[..n]);
    }
    let mut section=Vec::with_capacity(8+dep.len()+intent.len());
    section.extend_from_slice(&(dep.len() as u32).to_le_bytes());
    section.extend_from_slice(&(intent.len() as u32).to_le_bytes());
    section.extend_from_slice(&dep);
    section.extend_from_slice(&intent);

    let header_end=RAPP_V3_HEADER_SIZE;
    let mut b=Vec::with_capacity(header_end+capabilities.len()*2+section.len()+elf_payload.len());
    b.extend_from_slice(RAPP_MAGIC); b.extend_from_slice(&RAPP_FORMAT_VERSION_V4.to_le_bytes()); b.extend_from_slice(&(arch as u16).to_le_bytes());
    let mut pkg=[0u8;32]; let n=package_id.as_bytes().len().min(32); pkg[..n].copy_from_slice(&package_id.as_bytes()[..n]); b.extend_from_slice(&pkg);
    b.extend_from_slice(publisher_key); b.extend_from_slice(&version.0.to_le_bytes()); b.extend_from_slice(&version.1.to_le_bytes()); b.extend_from_slice(&version.2.to_le_bytes()); b.extend_from_slice(&(capabilities.len() as u16).to_le_bytes());
    b.extend_from_slice(&digest); b.extend_from_slice(signature); b.extend_from_slice(&(elf_payload.len() as u32).to_le_bytes()); b.extend_from_slice(&(section.len() as u32).to_le_bytes());
    for &cap in capabilities{b.extend_from_slice(&cap.to_le_bytes())}
    b.extend_from_slice(&section); b.extend_from_slice(elf_payload); b
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
        create_signed_test_package_with_id("org.interos.clock", caps, seed)
    }

    fn create_signed_test_package_with_id(package_id: &str, caps: &[u16], seed: u8) -> (Vec<u8>, [u8; 32], SigningKey) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let pub_key = signing_key.verifying_key().to_bytes();
        let payload = b"\x7fELF_USER_BINARY_SAMPLE";
        let placeholder_sig = [0u8; 64];

        let mut pkg = build_rapp(
            package_id,
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
    fn parse_rapp_accepts_all_capabilities_1_through_30() {
        // WHY 30: 29 is MemoryHeap and 30 is ProcessThreads. The table is the ABI,
        // and a package that asks for a capability the kernel has must be installable.
        let all_caps: alloc::vec::Vec<u16> = (1..=30).collect();
        let (pkg, pub_key, _) = create_signed_test_package(&all_caps, 20);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "all_caps_test").unwrap();

        let manifest = parse_rapp(&pkg).expect("package requesting IDs 1..=30 must parse");
        assert_eq!(manifest.capabilities, all_caps);
    }

    #[test]
    fn parse_rapp_rejects_out_of_bounds_capability_ids() {
        // Negative test: 0 is rejected
        let (pkg0, pub0, _) = create_signed_test_package(&[0], 21);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub0, "cap_zero").unwrap();
        assert_eq!(parse_rapp(&pkg0).unwrap_err(), RappError::UnknownCapability);

        // Negative test: 31 is the first identifier above the table (1..=30 are valid)
        let (pkg31, pub31, _) = create_signed_test_package(&[31], 22);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub31, "cap_31").unwrap();
        assert_eq!(parse_rapp(&pkg31).unwrap_err(), RappError::UnknownCapability);
    }

    #[test]
    fn a_v4_package_carries_its_declared_handlers() {
        // WHY a package that declares a handler can be installed: the
        // declaration is part of the signed manifest, so the registry can be
        // built from what was installed rather than from what an app says.
        let signing_key = SigningKey::from_bytes(&[31; 32]);
        let pub_key = signing_key.verifying_key().to_bytes();
        let payload = b"\x7fELF_USER_BINARY_SAMPLE";
        let declarations = [
            IntentDeclaration::new(IntentKind::Mime, "image/png"),
            IntentDeclaration::new(IntentKind::Action, "org.interos.action.set_wallpaper"),
        ];
        let mut pkg = build_rapp_v4(
            "org.interos.gallery",
            &pub_key,
            (1, 0, 0),
            RappArch::X86_64,
            &[1, 25, 26],
            &[],
            &declarations,
            payload,
            &[0u8; 64],
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "v4_intent_test").unwrap();

        // The signature covers the intent table, so the table is read out of
        // the signed bytes and not out of a side channel.
        let section_len = u32::from_le_bytes([pkg[0xB4], pkg[0xB5], pkg[0xB6], pkg[0xB7]]) as usize;
        let section_start = RAPP_V3_HEADER_SIZE + 3 * 2;
        let payload_offset = section_start + section_len;
        let msg = signed_message(&pkg, payload_offset, payload.len());
        let sig = signing_key.sign(&msg);
        pkg[RAPP_SIGNATURE_OFFSET..RAPP_SIGNATURE_OFFSET + RAPP_SIGNATURE_SIZE]
            .copy_from_slice(&sig.to_bytes());

        let manifest = parse_rapp(&pkg).expect("a v4 package with declared handlers must parse");
        assert_eq!(manifest.format_version, RAPP_FORMAT_VERSION_V4);
        assert_eq!(manifest.intents.len(), 2);
        assert_eq!(manifest.intents[0].kind, IntentKind::Mime);
        assert_eq!(manifest.intents[0].value, "image/png");
        assert_eq!(manifest.intents[1].kind, IntentKind::Action);
        assert_eq!(manifest.intents[1].value, "org.interos.action.set_wallpaper");
    }

    #[test]
    fn a_v4_package_with_no_declarations_still_parses() {
        let signing_key = SigningKey::from_bytes(&[32; 32]);
        let pub_key = signing_key.verifying_key().to_bytes();
        let payload = b"\x7fELF_USER_BINARY_SAMPLE";
        let mut pkg = build_rapp_v4(
            "org.interos.plain",
            &pub_key,
            (1, 0, 0),
            RappArch::X86_64,
            &[1],
            &[],
            &[],
            payload,
            &[0u8; 64],
        );
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "v4_empty_test").unwrap();
        let section_len = u32::from_le_bytes([pkg[0xB4], pkg[0xB5], pkg[0xB6], pkg[0xB7]]) as usize;
        let payload_offset = RAPP_V3_HEADER_SIZE + 2 + section_len;
        let msg = signed_message(&pkg, payload_offset, payload.len());
        let sig = signing_key.sign(&msg);
        pkg[RAPP_SIGNATURE_OFFSET..RAPP_SIGNATURE_OFFSET + RAPP_SIGNATURE_SIZE]
            .copy_from_slice(&sig.to_bytes());

        let manifest = parse_rapp(&pkg).expect("a v4 package with no handlers must parse");
        assert!(manifest.intents.is_empty());
    }

    #[test]
    fn capabilities_from_rapp_least_privilege_and_scoping() {
        use crate::capability::{CapabilityType, Rights};
        use crate::services::*;

        let all_ids: alloc::vec::Vec<u16> = (1..=24).collect();
        // Negative test: Untrusted app requesting capability 18 must be strictly denied
        assert!(
            crate::task::user::capabilities_from_rapp(&[18], 0x1234).is_err(),
            "Untrusted app requesting capability 18 must be strictly rejected"
        );

        // System authority requesting capabilities succeeds
        let table = crate::task::user::capabilities_from_rapp_with_trust(&all_ids, 0x1234, true)
            .expect("capabilities_from_rapp_with_trust for 1..=24 must succeed with system authority");

        // Verify ID 18 (Global SystemControl)
        let cap18 = table.get(17).expect("slot 17 for ID 18");
        assert_eq!(cap18.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap18.rights, Rights::INVOKE);
        assert_eq!(cap18.object_id, 0);

        // Verify ID 19 (Power SystemControl)
        let cap19 = table.get(18).expect("slot 18 for ID 19");
        assert_eq!(cap19.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap19.rights, Rights::INVOKE);
        assert_eq!(cap19.object_id, SERVICE_POWER_CONTROL_OBJECT);

        // Verify ID 20 (Telephony SystemControl)
        let cap20 = table.get(19).expect("slot 19 for ID 20");
        assert_eq!(cap20.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap20.rights, Rights::INVOKE);
        assert_eq!(cap20.object_id, SERVICE_TELEPHONY_CONTROL_OBJECT);

        // Verify ID 21 (Audio SystemControl)
        let cap21 = table.get(20).expect("slot 20 for ID 21");
        assert_eq!(cap21.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap21.rights, Rights::INVOKE);
        assert_eq!(cap21.object_id, SERVICE_AUDIO_CONTROL_OBJECT);

        // Verify ID 22 (Camera SystemControl)
        let cap22 = table.get(21).expect("slot 21 for ID 22");
        assert_eq!(cap22.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap22.rights, Rights::INVOKE);
        assert_eq!(cap22.object_id, SERVICE_CAMERA_CONTROL_OBJECT);

        // Verify ID 23 (Sensor SystemControl)
        let cap23 = table.get(22).expect("slot 22 for ID 23");
        assert_eq!(cap23.cap_type, CapabilityType::SystemControl);
        assert_eq!(cap23.rights, Rights::INVOKE);
        assert_eq!(cap23.object_id, SERVICE_SENSOR_CONTROL_OBJECT);

        // Verify ID 24 (RealtimeClockRead)
        let cap24 = table.get(23).expect("slot 23 for ID 24");
        assert_eq!(cap24.cap_type, CapabilityType::RealtimeClockRead);
        assert_eq!(cap24.rights, Rights::READ);
        assert_eq!(cap24.object_id, 0);

        // Verify ID 28 (AudioStream): PCM output is its own grant, not a
        // SystemControl domain, so a player holds only what it needs.
        let audio_table = crate::task::user::capabilities_from_rapp(&[28], 0)
            .expect("ID 28 must map to AudioStream");
        let cap28 = audio_table.get(0).expect("slot 0 for ID 28");
        assert_eq!(cap28.cap_type, CapabilityType::AudioStream);
        assert_eq!(cap28.rights, Rights::READ | Rights::WRITE);
        assert_eq!(cap28.object_id, 0);

        // Verify ID 29 (MemoryHeap) and 30 (ProcessThreads)
        let heap_table = crate::task::user::capabilities_from_rapp(&[29], 0)
            .expect("ID 29 must map to MemoryHeap");
        let cap29 = heap_table.get(0).expect("slot 0 for ID 29");
        assert_eq!(cap29.cap_type, CapabilityType::MemoryHeap);
        assert_eq!(cap29.rights, Rights::READ | Rights::WRITE);

        let thread_table = crate::task::user::capabilities_from_rapp(&[30], 0)
            .expect("ID 30 must map to ProcessThreads");
        let cap30 = thread_table.get(0).expect("slot 0 for ID 30");
        assert_eq!(cap30.cap_type, CapabilityType::ProcessThreads);
        assert_eq!(cap30.rights, Rights::INVOKE);

        // Negative tests: unknown IDs fail closed
        assert!(crate::task::user::capabilities_from_rapp(&[0], 0).is_err());
        assert!(crate::task::user::capabilities_from_rapp(&[31], 0).is_err());
        assert!(crate::task::user::capabilities_from_rapp(&[999], 0).is_err());
    }

    #[test]
    fn parse_rapp_rejects_untrusted_publisher() {
        let (pkg, _pub_key, _) = create_signed_test_package(&[1], 12);
        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::PublisherUntrusted);
    }

    #[test]
    fn parse_rapp_rejects_revoked_key() {
        let pkg_id = "org.interos.revoked_key_test";
        let (pkg, pub_key, _) = create_signed_test_package_with_id(pkg_id, &[1], 13);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "revoked_pub").unwrap();
        REVOCATION_REGISTRY.lock().place_key_on_crl(&pub_key, RevocationReason::KeyCompromised, 1, "root").unwrap();

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::PublisherRevoked);
        assert_eq!(inspect_rapp_trust(&pkg), TrustState::Revoked);

        REVOCATION_REGISTRY.lock().revoked_keys.remove(&pub_key);
        REVOCATION_REGISTRY.lock().trusted_keys.remove(&pub_key);
    }

    #[test]
    fn parse_rapp_accepts_valid_signed_package_with_trusted_key() {
        let pkg_id = "org.interos.trusted_test";
        let (pkg, pub_key, _) = create_signed_test_package_with_id(pkg_id, &[1, 24], 50);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "trusted_dev").unwrap();

        let manifest = parse_rapp(&pkg).expect("trusted package must parse successfully");
        assert_eq!(manifest.trust_state, TrustState::Trusted);
        assert_eq!(manifest.package_id, pkg_id);
        assert_eq!(inspect_rapp_trust(&pkg), TrustState::Trusted);

        REVOCATION_REGISTRY.lock().trusted_keys.remove(&pub_key);
    }

    #[test]
    fn parse_rapp_rejects_revoked_package() {
        let pkg_id = "org.interos.revoked_pkg_test";
        let (pkg, pub_key, _) = create_signed_test_package_with_id(pkg_id, &[1], 51);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "revoked_pkg_test").unwrap();
        REVOCATION_REGISTRY.lock().place_package_on_crl(pkg_id, RevocationReason::MaliciousBehavior, 1, "root").unwrap();

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::PackageRevoked);
        assert_eq!(inspect_rapp_trust(&pkg), TrustState::Revoked);

        REVOCATION_REGISTRY.lock().revoked_packages.remove(pkg_id);
        REVOCATION_REGISTRY.lock().trusted_keys.remove(&pub_key);
    }

    #[test]
    fn parse_rapp_rejects_tampered_payload_digest_mismatch() {
        let (mut pkg, pub_key, _) = create_signed_test_package(&[1], 52);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "tamper_test").unwrap();

        // Tamper with payload byte
        let last_idx = pkg.len() - 1;
        pkg[last_idx] ^= 0xAA;

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::DigestMismatch);
        assert_eq!(inspect_rapp_trust(&pkg), TrustState::Tampered);
        REVOCATION_REGISTRY.lock().trusted_keys.remove(&pub_key);
    }

    #[test]
    fn parse_rapp_rejects_tampered_header_signature_invalid() {
        let (mut pkg, pub_key, _) = create_signed_test_package(&[1], 53);
        REVOCATION_REGISTRY.lock().grant_trust_root(pub_key, "sig_tamper_test").unwrap();

        // Tamper with signature
        pkg[RAPP_SIGNATURE_OFFSET] ^= 0x55;

        let res = parse_rapp(&pkg);
        assert_eq!(res.unwrap_err(), RappError::SignatureInvalid);
        assert_eq!(inspect_rapp_trust(&pkg), TrustState::Tampered);
        REVOCATION_REGISTRY.lock().trusted_keys.remove(&pub_key);
    }

    #[test]
    fn inspect_rapp_trust_reports_discrete_typed_states() {
        // Untrusted
        let (pkg_untrusted, _, _) = create_signed_test_package(&[1], 54);
        assert_eq!(inspect_rapp_trust(&pkg_untrusted), TrustState::Untrusted);

        // Corrupted container
        assert_eq!(inspect_rapp_trust(b"NOT_A_RAPP_FILE"), TrustState::Tampered);

        // State conversions as_u8 and from_u8
        assert_eq!(TrustState::from_u8(TrustState::Trusted.as_u8()), Some(TrustState::Trusted));
        assert_eq!(TrustState::from_u8(TrustState::Untrusted.as_u8()), Some(TrustState::Untrusted));
        assert_eq!(TrustState::from_u8(TrustState::Revoked.as_u8()), Some(TrustState::Revoked));
        assert_eq!(TrustState::from_u8(TrustState::Tampered.as_u8()), Some(TrustState::Tampered));
        assert_eq!(TrustState::from_u8(4), None);
    }
}