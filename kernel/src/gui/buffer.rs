//! Display buffer ownership and frame publication contract.
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferState { Free, Drawing, Ready, Displayed }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameFence { pub sequence:u64, pub signaled:bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayBuffer { pub id:BufferId, pub owner:u64, pub width:u32, pub height:u32, pub state:BufferState, pub fence:FrameFence }

pub struct BufferManager { buffers:Vec<DisplayBuffer>, next_id:u32, next_sequence:u64 }
impl BufferManager {
 pub fn new()->Self{Self{buffers:Vec::new(),next_id:1,next_sequence:1}}
 pub fn allocate(&mut self,owner:u64,width:u32,height:u32)->Option<BufferId>{if width==0||height==0||self.buffers.len()>=8{return None}let id=BufferId(self.next_id);self.next_id=self.next_id.wrapping_add(1).max(1);self.buffers.push(DisplayBuffer{id,owner,width,height,state:BufferState::Free,fence:FrameFence{sequence:0,signaled:true}});Some(id)}
 pub fn begin_frame(&mut self,id:BufferId,owner:u64)->Result<(),&'static str>{let b=self.get_mut(id,owner)?;if b.state!=BufferState::Free{return Err("buffer not free")};b.state=BufferState::Drawing;b.fence=FrameFence{sequence:self.next_sequence,signaled:false};self.next_sequence=self.next_sequence.wrapping_add(1);Ok(())}
 pub fn publish(&mut self,id:BufferId,owner:u64)->Result<FrameFence,&'static str>{let b=self.get_mut(id,owner)?;if b.state!=BufferState::Drawing{return Err("buffer not drawing")};b.state=BufferState::Ready;b.fence.signaled=true;Ok(b.fence)}
 pub fn display(&mut self,id:BufferId)->Result<(),&'static str>{let b=self.buffers.iter_mut().find(|b|b.id==id).ok_or("buffer not found")?;if b.state!=BufferState::Ready{return Err("buffer not ready")};b.state=BufferState::Displayed;Ok(())}
 pub fn release(&mut self,id:BufferId,owner:u64)->Result<(),&'static str>{let b=self.get_mut(id,owner)?;if b.state==BufferState::Displayed||b.state==BufferState::Ready{b.state=BufferState::Free;Ok(())}else{Err("buffer is in use")}}
 fn get_mut(&mut self,id:BufferId,owner:u64)->Result<&mut DisplayBuffer,&'static str>{let b=self.buffers.iter_mut().find(|b|b.id==id).ok_or("buffer not found")?;if b.owner!=owner{return Err("buffer ownership denied")};Ok(b)}
}
