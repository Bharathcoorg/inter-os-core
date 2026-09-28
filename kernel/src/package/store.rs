//! Persistent package payload store.
//! Payloads are kept separate from registry metadata and addressed by package ID.
use alloc::vec::Vec;
use spin::Mutex;

const MAX_BLOBS:usize=32;
const MAX_BLOB_SIZE:usize=60000;

#[derive(Clone)]
pub struct PackageBlob { pub package_id:String, pub version:(u16,u16,u16), pub bytes:Vec<u8> }

pub struct PackageBlobStore { blobs:Vec<PackageBlob> }
impl PackageBlobStore {
 pub const fn new()->Self{Self{blobs:Vec::new()}}
 pub fn put(&mut self,package_id:&str,version:(u16,u16,u16),bytes:&[u8])->Result<(),()>{
  if bytes.is_empty()||bytes.len()>MAX_BLOB_SIZE{return Err(());}
  let previous=self.blobs.clone();
  if let Some(b)=self.blobs.iter_mut().find(|b|b.package_id==package_id){b.version=version;b.bytes.clear();b.bytes.extend_from_slice(bytes);}
  else {if self.blobs.len()>=MAX_BLOBS{return Err(());}self.blobs.push(PackageBlob{package_id:package_id.to_string(),version,bytes:bytes.to_vec()});}
  if self.persist().is_err() {
   self.blobs=previous;
   return Err(());
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
 pub fn restore(&mut self){
  let mut b=Vec::with_capacity(65472); b.resize(65472,0);let Ok(Some((_g,len)))=crate::storage::persistent::load_latest_kind(&mut b,5)else{return};
  if len<5||&b[..4]!=b"PKG1"{return}let mut p=5;let n=b[4].min(MAX_BLOBS as u8)as usize;
  for _ in 0..n{if p>=len{break}let il=b[p]as usize;p+=1;if il==0||p+il+10>len{break}let id=match core::str::from_utf8(&b[p..p+il]).ok(){Some(v)=>v,None=>break};p+=il;
   let v=(u16::from_le_bytes([b[p],b[p+1]]),u16::from_le_bytes([b[p+2],b[p+3]]),u16::from_le_bytes([b[p+4],b[p+5]]));p+=6;
   let dl=u32::from_le_bytes([b[p],b[p+1],b[p+2],b[p+3]])as usize;p+=4;if p+dl>len||dl>MAX_BLOB_SIZE{break}
   if id.len() > 128 { break; } self.blobs.push(PackageBlob{package_id:id.to_string(),version:v,bytes:b[p..p+dl].to_vec()});p+=dl;
  }
 }
}
pub static PACKAGE_BLOB_STORE:Mutex<PackageBlobStore>=Mutex::new(PackageBlobStore::new());
