#![no_std]
#![no_main]
use core::panic::PanicInfo;
use interos_std as std;
const W:usize=720; const H:usize=1440;
fn fill_row(row:&mut [u32;W],y:usize,packages:usize,windows:usize,services:usize){
 row.fill(if y<96{std::design::color::SURFACE}else{std::design::color::BACKGROUND});
 if y<96{let activity=((std::get_ticks(2).unwrap_or(0)%600)as usize).min(W-48);for x in 24..24+activity{row[x]=std::design::color::ACCENT;}}
 if (112..232).contains(&y){for x in 24..W-24{row[x]=std::design::color::SURFACE;}}
 if y==248{for x in 24..W-24{row[x]=std::design::color::BORDER;}}
 let rows=((packages.max(1)+1)/2).min(4);
 for card in 0..rows*2{let col=card%2;let r=card/2;let x0=56+col*304;let y0=400+r*180;if y>=y0&&y<y0+130{for x in x0..x0+250{row[x]=std::design::color::SURFACE;}}if y>=y0+20&&y<y0+84{for x in x0+93..x0+157{row[x]=std::design::color::ACCENT;}}}
 if (1170..1240).contains(&y){for x in 24..W-24{row[x]=std::design::color::SURFACE_RAISED;}}
 if (1270..1400).contains(&y){for x in 24..W-24{row[x]=std::design::color::SURFACE;}}
 for (i,n) in [windows.min(8),services.min(8)].iter().enumerate(){let x0=48+i*300;for k in 0..*n{for x in x0+k*24..x0+k*24+14{row[x]=std::design::color::SUCCESS;}}}
}
fn render(surface:u32,p:usize,w:usize,s:usize){let mut row=[0u32;W];for y in 0..H{fill_row(&mut row,y,p,w,s);let _=std::surface_write(4,surface,(y*W)as usize,&row);}}
fn first_launchable(c:&[u8])->Option<&[u8]>{let mut start=0;while start<c.len(){let end=c[start..].iter().position(|b|*b==b'\n').map(|p|start+p).unwrap_or(c.len());let line=&c[start..end];if line.starts_with(b"org.interos.settings|")||line.starts_with(b"org.interos.terminal|"){if let Some(p)=line.iter().position(|b|*b==b'|'){return Some(&line[..p]);}}if end==c.len(){break}start=end+1;}None}
#[no_mangle]pub extern "C" fn _start()->!{std::log(0,"[LAUNCHER USERSPACE] Live launcher online.").ok();let surface=match std::surface_create(4,W as u32,H as u32,0){Ok(id)=>id,Err(_)=>std::exit_with_code(1)};let mut catalog=[0u8;2048];let mut windows=[0u8;4096];let mut services=[0u8;1024];let n=std::package_list(3,&mut catalog).unwrap_or(0);let p=&catalog[..n];let package_count=p.iter().filter(|b|**b==b'\n').count();let wc=std::window_list(&mut windows).map(|n|if n>0{1}else{0}).unwrap_or(0);let sc=std::service_list(&mut services).map(|n|if n>0{1}else{0}).unwrap_or(0);render(surface,package_count,wc,sc);let _=std::surface_flip(4);let mut launched=false;loop{if let Ok(events)=std::event_poll(std::EVENT_TOUCH_INPUT|std::EVENT_TIMER_TICK|std::EVENT_IPC_MSG,100){if !launched&&(events&std::EVENT_TOUCH_INPUT)!=0{if let Some(id)=first_launchable(p){if std::ipc_send(1,2,0x5001,id).is_ok(){launched=true;}}}if events&std::EVENT_TIMER_TICK!=0{let wc=std::window_list(&mut windows).map(|n|if n>0{1}else{0}).unwrap_or(0);let sc=std::service_list(&mut services).map(|n|if n>0{1}else{0}).unwrap_or(0);render(surface,package_count,wc,sc);let _=std::surface_flip(4);}}let _=std::touch_poll();std::yield_now();}}
#[panic_handler]fn panic(_info:&PanicInfo)->!{std::exit();}