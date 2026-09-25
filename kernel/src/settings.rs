//! Persistent system settings service.
//!
//! Settings are authoritative kernel state mirrored to a crash-consistent ATA
//! journal. Userspace accesses them only through capability-checked syscalls.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

const MAX: usize = 4096;
const KIND: u8 = 3;

pub struct SettingsStore { data: Vec<u8>, generation: u64 }
impl SettingsStore {
    pub const fn new() -> Self { Self { data: Vec::new(), generation: 0 } }

    fn find(&self, key: &str) -> Option<(usize,usize)> {
        let mut start=0;
        while start<self.data.len() {
            let end=self.data[start..].iter().position(|b|*b==b'\n').map(|x|start+x).unwrap_or(self.data.len());
            let line=&self.data[start..end];
            if let Some(eq)=line.iter().position(|b|*b==b'=') {
                if core::str::from_utf8(&line[..eq]).ok()==Some(key) { return Some((start,end)); }
            }
            start=end.saturating_add(1);
        }
        None
    }

    pub fn get(&self, key:&str, out:&mut [u8])->usize {
        let Some((start,end))=self.find(key) else { return 0; };
        let line=&self.data[start..end];
        let eq=line.iter().position(|b|*b==b'=').unwrap_or(line.len());
        let value=&line[eq.saturating_add(1)..];
        let n=value.len().min(out.len()); out[..n].copy_from_slice(&value[..n]); n
    }

    pub fn set(&mut self, key:&str, value:&[u8])->Result<(),()> {
        if key.is_empty() || key.len()>96 || value.len()>512 || key.as_bytes().iter().any(|b|*b==b'\n'||*b==b'=') || value.iter().any(|b|*b==b'\n') { return Err(()); }
        let mut next=Vec::with_capacity(MAX);
        let replacement_len=key.len()+1+value.len()+1;
        let old=self.find(key);
        let mut pos=0;
        let mut replaced=false;
        while pos<self.data.len() {
            let end=self.data[pos..].iter().position(|b|*b==b'\n').map(|x|pos+x).unwrap_or(self.data.len());
            let is_old=old.map(|(s,e)|s==pos&&e==end).unwrap_or(false);
            if !is_old { if next.len()+end-pos+1>MAX{return Err(());} next.extend_from_slice(&self.data[pos..end]);next.push(b'\n'); }
            else { if next.len()+replacement_len>MAX{return Err(());} next.extend_from_slice(key.as_bytes());next.push(b'=');next.extend_from_slice(value);next.push(b'\n');replaced=true; }
            pos=end.saturating_add(1);
        }
        if !replaced { if next.len()+replacement_len>MAX{return Err(());} next.extend_from_slice(key.as_bytes());next.push(b'=');next.extend_from_slice(value);next.push(b'\n'); }
        let generation=crate::storage::persistent::commit_kind(&next,KIND).map_err(|_|())?;
        self.data=next; self.generation=generation; Ok(())
    }

    pub fn snapshot(&self,out:&mut[u8])->usize { let n=self.data.len().min(out.len());out[..n].copy_from_slice(&self.data[..n]);n }
}
pub static SETTINGS: Mutex<SettingsStore> = Mutex::new(SettingsStore::new());

pub fn init() {
    let mut buf=[0u8;MAX];
    let mut s=SETTINGS.lock();
    if let Ok(Some((generation,len)))=crate::storage::persistent::load_latest_kind(&mut buf,KIND) {
        s.data.extend_from_slice(&buf[..len]); s.generation=generation;
    } else {
        let defaults=b"power.profile=balanced\nconnectivity.cellular=true\nui.theme=interos-default\nui.brightness.auto=true\n";
        if let Ok(g)=crate::storage::persistent::commit_kind(defaults,KIND) { s.data.extend_from_slice(defaults); s.generation=g; }
    }
}
