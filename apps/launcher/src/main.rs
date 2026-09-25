#![no_std]
#![no_main]
use core::panic::PanicInfo;
use interos_std as std;
const W: usize=720; const H: usize=1440;
fn fill_row(row:&mut [u32;W],y:usize,count:usize){
 row.fill(if y<96{0xFF101827}else{0xFF070B12});
 if (112..232).contains(&y){for x in 24..W-24{row[x]=0xFF111B2B;}}
 if y==248{for x in 24..W-24{row[x]=0xFF26354D;}}
 let rows=((count.max(1)+1)/2).min(4);
 for card in 0..rows*2{let col=card%2;let r=card/2;let x0=56+col*304;let y0=400+r*180;
  if y>=y0&&y<y0+130{for x in x0..x0+250{row[x]=0xFF111B2B;}}
  if y>=y0+20&&y<y0+84{for x in x0+93..x0+157{row[x]=0xFF31C4FF;}}
 }
 if (1270..1400).contains(&y){for x in 24..W-24{row[x]=0xFF101827;}}
}
fn render(surface:u32,count:usize){let mut row=[0u32;W];for y in 0..H{fill_row(&mut row,y,count);let _=std::surface_write(surface,(y*W)as u32,&row);}}
fn first_launchable(c:&[u8])->Option<&[u8]>{let mut s=0;while s<c.len(){let e=match c[s..].iter().position(|b|*b==b'\n'){Some(p)=>s+p,None=>c.len()};let l=&c[s..e];if l.starts_with(b"org.interos.settings|")||l.starts_with(b"org.interos.terminal|"){if let Some(p)=l.iter().position(|b|*b==b'|'){return Some(&l[..p]);}}if e==c.len(){break}s=e+1;}None}
#[no_mangle]pub extern "C" fn _start()->!{
 std::log(0,"[LAUNCHER USERSPACE] Real Rust launcher service online.").ok();
 let surface=match std::surface_create(W as u32,H as u32,0){Ok(id)=>id,Err(_)=>{std::log(0,"[LAUNCHER USERSPACE] Launcher surface allocation failed.").ok();std::exit_with_code(1)}};
 let mut catalog_buf=[0u8;2048];let n=std::package_list(3,&mut catalog_buf).unwrap_or(0);let catalog=&catalog_buf[..n];
 let count=catalog.iter().filter(|b|**b==b'\n').count();
 std::log(0,"[LAUNCHER USERSPACE] Package catalog loaded from kernel package service.").ok();
 render(surface,count);let _=std::surface_flip();
 let mut launched=false;
 loop{if let Ok(events)=std::event_poll(std::EVENT_TOUCH_INPUT|std::EVENT_TIMER_TICK,100){if !launched&&(events&std::EVENT_TOUCH_INPUT)!=0{if let Some(id)=first_launchable(catalog){if std::ipc_send(1,2,0x5001,id).is_ok(){launched=true;std::log(0,"[LAUNCHER USERSPACE] Dynamic package launch request accepted.").ok();}}}}let _=std::touch_poll();std::yield_now();}
}
#[panic_handler]fn panic(_info:&PanicInfo)->!{std::log(0,"[LAUNCHER USERSPACE] Panic; terminating launcher task.").ok();std::exit();}
