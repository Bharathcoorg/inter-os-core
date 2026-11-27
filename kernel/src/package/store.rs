// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Persistent package payload store.
//! Payloads are kept separate from registry metadata and addressed by package ID.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

const MAX_BLOBS:usize=32;
pub const MAX_BLOB_SIZE:usize=60000;
pub const MAX_RUNTIME_ARTIFACT_SIZE:usize = 16 * 1024 * 1024;
/// Bounded total byte quota across all entries in `PackageBlobStore` (leaves safe
/// headroom inside the 64 MiB kernel heap against cumulative allocation DoS).
pub const MAX_TOTAL_STORE_BYTES:usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct PackageBlob { pub package_id:String, pub version:(u16,u16,u16), pub code_digest:[u8; 32], pub bytes:Vec<u8> }

pub struct PackageBlobStore { blobs:Vec<PackageBlob> }
impl PackageBlobStore {
 pub const fn new()->Self{Self{blobs:Vec::new()}}
 fn total_bytes(&self)->usize{
  self.blobs.iter().fold(0usize,|acc,b|acc.saturating_add(b.bytes.len()))
 }
 pub fn put(&mut self,package_id:&str,version:(u16,u16,u16),bytes:&[u8])->Result<(),()>{
  if bytes.is_empty()||bytes.len()>MAX_BLOB_SIZE{return Err(());}
  let code_digest = crate::crypto::Sha256::digest(bytes);
  let previous=self.blobs.clone();
  if let Some(b)=self.blobs.iter_mut().find(|b|b.package_id==package_id){b.version=version;b.code_digest=code_digest;b.bytes.clear();b.bytes.extend_from_slice(bytes);}
  else {if self.blobs.len()>=MAX_BLOBS{return Err(());}self.blobs.push(PackageBlob{package_id:package_id.to_string(),version,code_digest,bytes:bytes.to_vec()});}
  if self.persist().is_err() {
   self.blobs=previous;
   return Err(());
  }
  Ok(())
 }
 pub fn put_verified(&mut self,package_id:&str,version:(u16,u16,u16),code_digest:[u8;32],bytes:&[u8])->Result<(),()>{
  if bytes.is_empty()||bytes.len()>MAX_RUNTIME_ARTIFACT_SIZE{return Err(());}
  let existing_len=self.blobs.iter().find(|b|b.package_id==package_id).map(|b|b.bytes.len()).unwrap_or(0);
  if existing_len==0 && self.blobs.len()>=MAX_BLOBS{return Err(());}
  let new_total=self.total_bytes().saturating_sub(existing_len).checked_add(bytes.len()).ok_or(())?;
  if new_total>MAX_TOTAL_STORE_BYTES{return Err(());}
  let previous=self.blobs.clone();
  if let Some(b)=self.blobs.iter_mut().find(|b|b.package_id==package_id){b.version=version;b.code_digest=code_digest;b.bytes.clear();b.bytes.extend_from_slice(bytes);}
  else {self.blobs.push(PackageBlob{package_id:package_id.to_string(),version,code_digest,bytes:bytes.to_vec()});}
  // Fail closed if persistence fails (including payloads exceeding the journal
  // record capacity) rather than silently drifting between registry and blob store.
  if self.persist().is_err() {
   self.blobs=previous;
   return Err(());
  }
  Ok(())
 }
 pub fn put_runtime(&mut self,package_id:&str,version:(u16,u16,u16),bytes:&[u8])->Result<(),()>{
  if bytes.is_empty()||bytes.len()>MAX_RUNTIME_ARTIFACT_SIZE{return Err(());}
  let existing_idx=self.blobs.iter().position(|b|b.package_id==package_id);
  let existing_len=existing_idx.map(|i|self.blobs[i].bytes.len()).unwrap_or(0);
  if existing_idx.is_none() && self.blobs.len()>=MAX_BLOBS{return Err(());}
  let new_total=self.total_bytes().saturating_sub(existing_len).checked_add(bytes.len()).ok_or(())?;
  if new_total>MAX_TOTAL_STORE_BYTES{return Err(());}
  let code_digest = crate::crypto::Sha256::digest(bytes);
  if let Some(idx)=existing_idx{
   let b=&mut self.blobs[idx];
   b.version=version;
   b.code_digest=code_digest;
   b.bytes.clear();
   b.bytes.extend_from_slice(bytes);
  } else {
   self.blobs.push(PackageBlob{package_id:package_id.to_string(),version,code_digest,bytes:bytes.to_vec()});
  }
  if bytes.len()<=MAX_BLOB_SIZE {
   let _ = self.persist();
  }
  Ok(())
 }
 pub fn append_runtime(&mut self,package_id:&str,chunk:&[u8])->Result<(),()>{
  if chunk.is_empty(){return Err(());}
  let idx=self.blobs.iter().position(|b|b.package_id==package_id).ok_or(())?;
  let new_blob_len=self.blobs[idx].bytes.len().checked_add(chunk.len()).ok_or(())?;
  if new_blob_len>MAX_RUNTIME_ARTIFACT_SIZE{return Err(());}
  let new_total=self.total_bytes().checked_add(chunk.len()).ok_or(())?;
  if new_total>MAX_TOTAL_STORE_BYTES{return Err(());}
  let b=&mut self.blobs[idx];
  b.bytes.extend_from_slice(chunk);
  b.code_digest = crate::crypto::Sha256::digest(&b.bytes);
  if b.bytes.len()<=MAX_BLOB_SIZE {
   let _ = self.persist();
  }
  Ok(())
 }
 pub fn get(&self,id:&str)->Option<&PackageBlob>{self.blobs.iter().find(|b|b.package_id==id)} pub fn remove(&mut self,id:&str)->Result<bool,()>{
  let Some(pos)=self.blobs.iter().position(|b|b.package_id==id) else {return Ok(false);};
  let previous=self.blobs.clone();
  self.blobs.remove(pos);
  if self.persist().is_err(){self.blobs=previous;return Err(());}
  Ok(true)
 }
 fn persist(&self)->Result<(),()>{
  let mut out=Vec::with_capacity(65472); out.resize(65472,0);out[..4].copy_from_slice(b"PKG1");out[4]=self.blobs.len()as u8;let mut p=5;
  for b in &self.blobs{let id=b.package_id.as_bytes();if id.len()>63||p+1+id.len()+6+4+b.bytes.len()>out.len(){return Err(());}
   out[p]=id.len()as u8;p+=1;out[p..p+id.len()].copy_from_slice(id);p+=id.len();
   out[p..p+2].copy_from_slice(&b.version.0.to_le_bytes());out[p+2..p+4].copy_from_slice(&b.version.1.to_le_bytes());out[p+4..p+6].copy_from_slice(&b.version.2.to_le_bytes());p+=6;
   out[p..p+4].copy_from_slice(&(b.bytes.len()as u32).to_le_bytes());p+=4;out[p..p+b.bytes.len()].copy_from_slice(&b.bytes);p+=b.bytes.len();
  }crate::storage::persistent::commit_kind(&out[..p],5).map(|_|()).map_err(|_|())
 }

 #[cfg(test)]
 pub fn insert_for_test(&mut self, package_id: &str, version: (u16, u16, u16), bytes: &[u8]) {
     let code_digest = crate::crypto::Sha256::digest(bytes);
     self.blobs.retain(|b| b.package_id != package_id);
     self.blobs.push(PackageBlob {
         package_id: package_id.to_string(),
         version,
         code_digest,
         bytes: bytes.to_vec(),
     });
 }

 #[cfg(test)]
 pub fn remove_for_test(&mut self, package_id: &str) {
     self.blobs.retain(|b| b.package_id != package_id);
 }

 pub fn restore(&mut self){
  let mut b=Vec::with_capacity(65472);
  b.resize(65472,0);
  let Ok(Some((_g,len)))=crate::storage::persistent::load_latest_kind(&mut b,5) else{return};
  if len<5||&b[..4]!=b"PKG1"{return}

  let n=b[4] as usize;
  if n>MAX_BLOBS { return; }

  let mut restored=Vec::with_capacity(n);
  let mut p=5usize;
  for _ in 0..n {
   if p>=len { return; }
   let il=b[p] as usize; p+=1;
   if il==0 || il>crate::package::rapp::RAPP_MAX_PACKAGE_ID_LEN { return; }
   if p.checked_add(il+10).map(|end|end>len).unwrap_or(true) { return; }
   let id=match core::str::from_utf8(&b[p..p+il]).ok(){Some(v)=>v,None=>return};
   if id.contains('/') || id.contains('\\') || id.contains("..") { return; }
   if restored.iter().any(|entry: &PackageBlob| entry.package_id==id) { return; }
   p+=il;

   let v=(
    u16::from_le_bytes([b[p],b[p+1]]),
    u16::from_le_bytes([b[p+2],b[p+3]]),
    u16::from_le_bytes([b[p+4],b[p+5]])
   ); p+=6;
   let dl=u32::from_le_bytes([b[p],b[p+1],b[p+2],b[p+3]]) as usize; p+=4;
   if dl==0 || dl>MAX_BLOB_SIZE || p.checked_add(dl).map(|end|end>len).unwrap_or(true) { return; }

   let blob_bytes = b[p..p+dl].to_vec();
   let code_digest = crate::crypto::Sha256::digest(&blob_bytes);
   restored.push(PackageBlob{package_id:id.to_string(),version:v,code_digest,bytes:blob_bytes});
   p+=dl;
  }

  if p!=len { return; }
  self.blobs=restored;
 }
}
pub static PACKAGE_BLOB_STORE:Mutex<PackageBlobStore>=Mutex::new(PackageBlobStore::new());

/// Temporary staged candidate package artifact awaiting verification by ACR.
#[derive(Clone)]
pub struct CandidateArtifact {
    pub handle: u32,
    pub caller_task_id: u64,
    pub package_name: String,
    pub expected_len: usize,
    pub bytes: Vec<u8>,
}

impl CandidateArtifact {
    pub fn compute_digest(&self) -> [u8; 32] {
        crate::crypto::Sha256::digest(&self.bytes)
    }
}

/// Out-of-band store for unverified candidate package artifacts.
pub struct CandidateArtifactStore {
    candidates: Vec<CandidateArtifact>,
    next_handle: u32,
}

impl CandidateArtifactStore {
    pub const fn new() -> Self {
        Self {
            candidates: Vec::new(),
            next_handle: 1,
        }
    }

    /// Allocates an out-of-band candidate staging session.
    pub fn allocate_session(&mut self, package_name: &str, expected_len: usize) -> Result<u32, ()> {
        if self.candidates.len() >= 8 || expected_len == 0 || expected_len > MAX_RUNTIME_ARTIFACT_SIZE {
            return Err(());
        }
        let h = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        self.candidates.push(CandidateArtifact {
            handle: h,
            caller_task_id: 0,
            package_name: String::from(package_name),
            expected_len,
            bytes: Vec::new(),
        });
        Ok(h)
    }

    /// Appends a raw chunk to an existing candidate staging session.
    pub fn append_chunk(&mut self, handle: u32, chunk: &[u8]) -> Result<(), ()> {
        let candidate = self.candidates.iter_mut().find(|c| c.handle == handle).ok_or(())?;
        if candidate.bytes.len().saturating_add(chunk.len()) > MAX_RUNTIME_ARTIFACT_SIZE {
            return Err(());
        }
        candidate.bytes.extend_from_slice(chunk);
        Ok(())
    }

    /// Returns a reference to a candidate artifact session by handle.
    pub fn get_candidate(&self, handle: u32) -> Option<&CandidateArtifact> {
        self.candidates.iter().find(|c| c.handle == handle)
    }

    /// Appends or writes a chunk into candidate storage.
    pub fn stage_chunk(
        &mut self,
        handle: u32,
        caller_task_id: u64,
        chunk: &[u8],
        append: bool,
    ) -> Result<u32, ()> {
        if chunk.is_empty() {
            return Err(());
        }
        if handle == 0 {
            if self.candidates.len() >= 8 {
                return Err(());
            }
            if chunk.len() > MAX_RUNTIME_ARTIFACT_SIZE {
                return Err(());
            }
            let h = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1).max(1);
            self.candidates.push(CandidateArtifact {
                handle: h,
                caller_task_id,
                package_name: String::new(),
                expected_len: chunk.len(),
                bytes: chunk.to_vec(),
            });
            Ok(h)
        } else {
            let candidate = self
                .candidates
                .iter_mut()
                .find(|c| c.handle == handle)
                .ok_or(())?;
            if candidate.caller_task_id != caller_task_id && caller_task_id != 0 {
                return Err(());
            }
            if append {
                if candidate.bytes.len().saturating_add(chunk.len()) > MAX_RUNTIME_ARTIFACT_SIZE {
                    return Err(());
                }
                candidate.bytes.extend_from_slice(chunk);
            } else {
                if chunk.len() > MAX_RUNTIME_ARTIFACT_SIZE {
                    return Err(());
                }
                candidate.bytes.clear();
                candidate.bytes.extend_from_slice(chunk);
            }
            Ok(handle)
        }
    }

    /// Reads a chunk from a staged candidate artifact for userspace verification.
    pub fn read_chunk(&self, handle: u32, offset: usize, out: &mut [u8]) -> Result<usize, ()> {
        let candidate = self.candidates.iter().find(|c| c.handle == handle).ok_or(())?;
        if offset >= candidate.bytes.len() {
            return Ok(0);
        }
        let available = candidate.bytes.len() - offset;
        let to_copy = core::cmp::min(available, out.len());
        out[..to_copy].copy_from_slice(&candidate.bytes[offset..offset + to_copy]);
        Ok(to_copy)
    }

    /// Consumes and removes a candidate artifact upon atomic registration promotion.
    pub fn take_candidate(&mut self, handle: u32) -> Option<CandidateArtifact> {
        let pos = self.candidates.iter().position(|c| c.handle == handle)?;
        Some(self.candidates.remove(pos))
    }

    /// Discards a candidate artifact.
    pub fn discard(&mut self, handle: u32) {
        self.candidates.retain(|c| c.handle != handle);
    }
}

pub static CANDIDATE_ARTIFACT_STORE: Mutex<CandidateArtifactStore> =
    Mutex::new(CandidateArtifactStore::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_runtime_enforces_max_artifact_size_and_total_quota() {
        let mut store = PackageBlobStore::new();

        // Exceeding MAX_RUNTIME_ARTIFACT_SIZE in a single put_runtime must fail before storing.
        let oversized = alloc::vec![0xAAu8; MAX_RUNTIME_ARTIFACT_SIZE + 1];
        assert!(store.put_runtime("pkg.oversized", (1, 0, 0), &oversized).is_err());
        assert!(store.get("pkg.oversized").is_none());

        // Two max-sized artifacts fit within MAX_TOTAL_STORE_BYTES (32 MiB).
        let half = alloc::vec![0x11u8; MAX_RUNTIME_ARTIFACT_SIZE];
        assert!(store.put_runtime("pkg.one", (1, 0, 0), &half).is_ok());
        assert!(store.put_runtime("pkg.two", (1, 0, 0), &half).is_ok());

        // Adding 1 more byte exceeds total store quota.
        assert!(store.put_runtime("pkg.three", (1, 0, 0), &[0x22]).is_err());
        assert!(store.get("pkg.three").is_none());

        // Replacing an existing entry within total quota succeeds.
        assert!(store.put_runtime("pkg.one", (1, 0, 1), &[0x33u8; 64]).is_ok());
        assert_eq!(store.get("pkg.one").unwrap().bytes.len(), 64);
    }

    #[test]
    fn append_runtime_enforces_max_artifact_size_and_total_quota_without_growing() {
        let mut store = PackageBlobStore::new();
        let initial = alloc::vec![0x44u8; MAX_RUNTIME_ARTIFACT_SIZE - 4];
        assert!(store.put_runtime("pkg.stream", (1, 0, 0), &initial).is_ok());

        // Appending 5 bytes exceeds MAX_RUNTIME_ARTIFACT_SIZE and must not grow the vec.
        assert!(store.append_runtime("pkg.stream", &[1, 2, 3, 4, 5]).is_err());
        assert_eq!(
            store.get("pkg.stream").unwrap().bytes.len(),
            MAX_RUNTIME_ARTIFACT_SIZE - 4
        );

        // Appending 4 bytes succeeds cleanly up to MAX_RUNTIME_ARTIFACT_SIZE.
        assert!(store.append_runtime("pkg.stream", &[1, 2, 3, 4]).is_ok());
        assert_eq!(
            store.get("pkg.stream").unwrap().bytes.len(),
            MAX_RUNTIME_ARTIFACT_SIZE
        );

        // Fill remaining quota with second artifact, then verify append respects total quota.
        let other = alloc::vec![0x55u8; MAX_RUNTIME_ARTIFACT_SIZE - 2];
        assert!(store.put_runtime("pkg.other", (1, 0, 0), &other).is_ok());
        assert!(store.append_runtime("pkg.other", &[9, 9, 9]).is_err());
        assert_eq!(
            store.get("pkg.other").unwrap().bytes.len(),
            MAX_RUNTIME_ARTIFACT_SIZE - 2
        );
    }

    #[test]
    fn put_verified_fails_closed_when_persist_cannot_succeed() {
        let mut store = PackageBlobStore::new();
        let bytes = alloc::vec![0x77u8; MAX_BLOB_SIZE + 1];
        let digest = crate::crypto::Sha256::digest(&bytes);

        // A payload larger than MAX_BLOB_SIZE cannot fit in the PKG1 journal record,
        // so put_verified must fail closed and leave the store untouched.
        assert!(store.put_verified("pkg.unpersistable", (1, 0, 0), digest, &bytes).is_err());
        assert!(store.get("pkg.unpersistable").is_none());

        // Even for a small payload, without an active persistent block device in unit tests,
        // persist() fails and put_verified must roll back and fail closed.
        let small = b"small-verified-blob";
        let small_digest = crate::crypto::Sha256::digest(small);
        assert!(store.put_verified("pkg.nodisk", (1, 0, 0), small_digest, small).is_err());
        assert!(store.get("pkg.nodisk").is_none());
    }
}
