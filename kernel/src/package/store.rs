//! Persistent package payload store.
//! Payloads are kept separate from registry metadata and addressed by package ID.
use alloc::vec::Vec;
use spin::Mutex;
use crate::package::first_party_package;

const MAX_BLOBS:usize=8;
const MAX_BLOB_SIZE:usize=24576;

#[derive(Clone)]
pub struct PackageBlob { pub package_id:&'static str, pub version:(u16,u16,u16), pub bytes:Vec<u8> }

pub struct PackageBlobStore { blobs:Vec<PackageBlob> }
impl PackageBlobStore {
 pub const fn new()->Self{Self{blobs:Vec::new()}}
 pub fn put(&mut self,package_id:&'static str,version:(u16,u16,u16),bytes:&[u8])->Result<(),()>{
  if bytes.is_empty()||bytes.len()>MAX_BLOB_SIZE{return Err(());}
  if let Some(b)=self.blobs.iter_mut().find(|b|b.package_id==package_id){b.version=version;b.bytes.clear();b.bytes.extend_from_slice(bytes);}
  else {if self.blobs.len()>=MAX_BLOBS{return Err(());}self.blobs.push(PackageBlob{package_id,version,bytes:bytes.to_vec()});}
  self.persist()
 }
 pub fn get(&self,id:&str)->Option<&PackageBlob>{self.blobs.iter().find(|b|b.package_id==id)}
 fn persist(&self)->Result<(),()>{
  let mut out=[0u8;32768];out[..4].copy_from_slice(b"PKG1");out[4]=self.blobs.len()as u8;let mut p=5;
  for b in &self.blobs{let id=b.package_id.as_bytes();if id.len()>63||p+1+id.len()+6+4+b.bytes.len()>out.len(){return Err(());}
   out[p]=id.len()as u8;p+=1;out[p..p+id.len()].copy_from_slice(id);p+=id.len();
   out[p..p+2].copy_from_slice(&b.version.0.to_le_bytes());out[p+2..p+4].copy_from_slice(&b.version.1.to_le_bytes());out[p+4..p+6].copy_from_slice(&b.version.2.to_le_bytes());p+=6;
   out[p..p+4].copy_from_slice(&(b.bytes.len()as u32).to_le_bytes());p+=4;out[p..p+b.bytes.len()].copy_from_slice(&b.bytes);p+=b.bytes.len();
  }crate::storage::persistent::commit_kind(&out[..p],5).map(|_|()).map_err(|_|())
 }
 pub fn restore(&mut self){
  let mut b=[0u8;32768];let Ok(Some((_g,len)))=crate::storage::persistent::load_latest_kind(&mut b,5)else{return};
  if len<5||&b[..4]!=b"PKG1"{return}let mut p=5;let n=b[4].min(MAX_BLOBS as u8)as usize;
  for _ in 0..n{if p>=len{break}let il=b[p]as usize;p+=1;if il==0||p+il+10>len{break}let id=match core::str::from_utf8(&b[p..p+il]).ok(){Some(v)=>v,None=>break};p+=il;
   let v=(u16::from_le_bytes([b[p],b[p+1]]),u16::from_le_bytes([b[p+2],b[p+3]]),u16::from_le_bytes([b[p+4],b[p+5]]));p+=6;
   let dl=u32::from_le_bytes([b[p],b[p+1],b[p+2],b[p+3]])as usize;p+=4;if p+dl>len||dl>MAX_BLOB_SIZE{break}
   if let Some(d)=first_party_package(id){self.blobs.push(PackageBlob{package_id:d.package_id,version:v,bytes:b[p..p+dl].to_vec()});}p+=dl;
  }
 }
}
pub static PACKAGE_BLOB_STORE:Mutex<PackageBlobStore>=Mutex::new(PackageBlobStore::new());
