//! Durable boot recovery policy for InterOS.
use spin::Mutex;
use crate::storage::persistent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootSlot { A=0, B=1 }
impl BootSlot { pub const fn other(self)->Self{match self{Self::A=>Self::B,Self::B=>Self::A}} }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootOutcome { Unknown, Pending, Success, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootRecord{pub active:BootSlot,pub last_good:BootSlot,pub outcome:BootOutcome,pub attempts:u8,pub max_attempts:u8}
impl BootRecord{
 pub const fn new(active:BootSlot)->Self{Self{active,last_good:active,outcome:BootOutcome::Unknown,attempts:0,max_attempts:3}}
 pub fn begin(&mut self){self.outcome=BootOutcome::Pending;self.attempts=self.attempts.saturating_add(1);}
 pub fn success(&mut self){self.last_good=self.active;self.outcome=BootOutcome::Success;self.attempts=0;}
 pub fn fail(&mut self){self.outcome=BootOutcome::Failed;}
 pub const fn should_recover(&self)->bool{matches!(self.outcome,BootOutcome::Failed|BootOutcome::Pending)&&self.attempts>=self.max_attempts}
 pub const fn recovery_slot(&self)->BootSlot{if self.should_recover(){self.last_good}else{self.active}}
 pub fn switch_to_other(&mut self){self.active=self.active.other();self.outcome=BootOutcome::Unknown;self.attempts=0;}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryError{Storage,CorruptRecord,NotDurable}
pub const RECOVERY_RECORD_KIND:u8=7;
const RECOVERY_RECORD_LEN:usize=16;
fn encode(record:&BootRecord)->[u8;RECOVERY_RECORD_LEN]{let mut out=[0u8;RECOVERY_RECORD_LEN];out[0]=record.active as u8;out[1]=record.last_good as u8;out[2]=record.outcome as u8;out[3]=record.attempts;out[4]=record.max_attempts;out}
fn decode(bytes:&[u8])->Option<BootRecord>{if bytes.len()!=RECOVERY_RECORD_LEN||bytes[0]>1||bytes[1]>1||bytes[2]>3||bytes[4]==0{return None;}Some(BootRecord{active:if bytes[0]==0{BootSlot::A}else{BootSlot::B},last_good:if bytes[1]==0{BootSlot::A}else{BootSlot::B},outcome:match bytes[2]{0=>BootOutcome::Unknown,1=>BootOutcome::Pending,2=>BootOutcome::Success,_=>BootOutcome::Failed},attempts:bytes[3],max_attempts:bytes[4]})}
pub struct BootRecovery{pub record:BootRecord,pub durable:bool}
impl BootRecovery{
 pub const fn new()->Self{Self{record:BootRecord::new(BootSlot::A),durable:false}}
 pub fn restore(&mut self)->Result<(),RecoveryError>{let mut buf=[0u8;RECOVERY_RECORD_LEN];match persistent::load_latest_kind(&mut buf,RECOVERY_RECORD_KIND){Ok(None)=>{self.durable=true;Ok(())}Ok(Some((_,len)))=>{self.record=decode(&buf[..len]).ok_or(RecoveryError::CorruptRecord)?;self.durable=true;Ok(())}Err(_)=>{self.durable=false;Err(RecoveryError::Storage)}}}
 fn persist_record(record:&BootRecord)->Result<(),RecoveryError>{persistent::commit_kind(&encode(record),RECOVERY_RECORD_KIND).map(|_|()).map_err(|_|RecoveryError::Storage)}
 fn commit_transition(&mut self,next:BootRecord)->Result<(),RecoveryError>{if !self.durable{return Err(RecoveryError::NotDurable);}Self::persist_record(&next)?;self.record=next;Ok(())}
 pub fn begin_boot(&mut self)->Result<(),RecoveryError>{let mut next=self.record;next.begin();self.commit_transition(next)}
 pub fn confirm_boot(&mut self)->Result<(),RecoveryError>{if self.record.outcome==BootOutcome::Success{return Ok(());}let mut next=self.record;next.success();self.commit_transition(next)}
 pub fn mark_failed(&mut self)->Result<(),RecoveryError>{let mut next=self.record;next.fail();self.commit_transition(next)}
 pub fn recover_last_good(&mut self)->Result<(),RecoveryError>{let mut next=self.record;next.active=next.last_good;next.outcome=BootOutcome::Unknown;next.attempts=0;self.commit_transition(next)}
}
#[cfg(test)]
mod tests{use super::*;#[test]fn failed_slot_recovers_to_last_good_after_bounded_attempts(){let mut r=BootRecord::new(BootSlot::A);r.switch_to_other();r.begin();r.fail();r.begin();r.fail();r.begin();r.fail();assert!(r.should_recover());assert_eq!(r.recovery_slot(),BootSlot::A);}#[test]fn recovery_slot_is_last_good_only_at_retry_limit(){let mut r=BootRecord::new(BootSlot::A);r.switch_to_other();r.begin();r.fail();assert_eq!(r.recovery_slot(),BootSlot::B);r.begin();r.fail();r.begin();r.fail();assert_eq!(r.recovery_slot(),BootSlot::A);}}
pub static BOOT_RECOVERY:Mutex<BootRecovery>=Mutex::new(BootRecovery::new());
pub fn init(){let mut recovery=BOOT_RECOVERY.lock();if let Err(error)=recovery.restore(){crate::serial_println!("[RECOVERY] durable boot state unavailable ({:?}); boot health confirmation is disabled.",error);return;}if recovery.record.should_recover(){if let Err(error)=recovery.recover_last_good(){crate::serial_println!("[RECOVERY] could not persist last-good recovery ({:?}); boot health confirmation is disabled.",error);return;}crate::serial_println!("[RECOVERY] recovered pending boot to last-good slot.");}if let Err(error)=recovery.begin_boot(){crate::serial_println!("[RECOVERY] could not persist boot attempt ({:?}); boot health confirmation is disabled.",error);}}
