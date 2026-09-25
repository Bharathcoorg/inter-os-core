#![no_std]
#![no_main]
use core::panic::PanicInfo;
use interos_std as std;
const W:u32=720;const H:u32=96;const CHUNK:usize=1024;
fn chunk(surface:u32,offset:usize,count:usize){let mut p=[0u32;CHUNK];p.fill(0xFF0B0F19);for i in 0..count{let q=offset+i;let x=q%(W as usize);let y=q/(W as usize);if y<2||y>=H as usize-2||x<2||x>=W as usize-2{p[i]=0xFF253044;}}let _=std::surface_write(surface,offset as u32,&p[..count]);}
#[no_mangle]pub extern "C"fn _start()->!{std::log(0,"[COMPOSITOR USERSPACE] Ring-3 compositor service online.").ok();match std::surface_create(W,H,100){Ok(surface)=>{let total=(W*H)as usize;let mut o=0;while o<total{let n=(total-o).min(CHUNK);chunk(surface,o,n);o+=n;}let _=std::surface_flip();std::log(0,"[COMPOSITOR USERSPACE] Bounded system-line frame committed.").ok();},Err(_)=>std::log(0,"[COMPOSITOR USERSPACE] Surface allocation unavailable; service remains alive.").ok()}loop{let _=std::event_poll(std::EVENT_TOUCH_INPUT|std::EVENT_TIMER_TICK|std::EVENT_IPC_MSG,100);let _=std::touch_poll();std::yield_now();}}
#[panic_handler]fn panic(_info:&PanicInfo)->!{std::log(0,"[COMPOSITOR USERSPACE] Panic; terminating compositor task.").ok();std::exit();}
