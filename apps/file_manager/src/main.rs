#![no_std]
#![no_main]
use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start()->!{
 std::log(0,"[FILE_MANAGER] InterOS VFS explorer online.").ok();
 let mut capacity=0u64;
 if let Ok(bytes)=std::storage_query(){capacity=bytes;std::log(0,"[FILE_MANAGER] Storage capacity read from live block device.").ok();}
 let mut entries=[0u8;2048];
 if let Ok(n)=std::fs_list(3,"/data",&mut entries){
   let _=std::log(0,core::str::from_utf8(&entries[..n]).unwrap_or("[FILE_MANAGER] Invalid VFS listing."));
 }
 if capacity==0{std::log(0,"[FILE_MANAGER] Storage capacity: UNAVAILABLE.").ok();}
 loop{if let Ok(events)=std::event_poll(std::EVENT_TIMER_TICK|std::EVENT_IPC_MSG,250){if events&std::EVENT_TIMER_TICK!=0{let _=std::storage_query();}}std::yield_now();}
}
#[panic_handler]fn panic(_info:&PanicInfo)->!{std::exit();}
