//! IP routing policy independent of the physical network device.
use super::ipv4::Ipv4Address;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Cidr { pub address: Ipv4Address, pub prefix_len: u8 }
impl Ipv4Cidr {
 pub fn contains(&self, ip:Ipv4Address)->bool { if self.prefix_len>32{return false} let mask=if self.prefix_len==0{0}else{u32::MAX<<(32-self.prefix_len)}; (u32::from_be_bytes(self.address.0)&mask)==(u32::from_be_bytes(ip.0)&mask) }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route { pub destination:Ipv4Cidr, pub gateway:Option<Ipv4Address>, pub interface:u8, pub metric:u16 }

pub const MAX_ROUTES:usize=32;
pub struct RoutingTable { routes:[Option<Route>;MAX_ROUTES] }
impl RoutingTable {
 pub const fn new()->Self{Self{routes:[None;MAX_ROUTES]}}
 pub fn add(&mut self,route:Route)->Result<(),Route>{
  if route.destination.prefix_len>32{return Err(route)}
  if let Some(slot)=self.routes.iter_mut().find(|r|r.is_none()){*slot=Some(route);Ok(())}else{Err(route)}
 }
 pub fn remove_interface(&mut self,interface:u8){for r in &mut self.routes{if r.map(|x|x.interface)==Some(interface){*r=None;}}}
 pub fn lookup(&self,dst:Ipv4Address)->Option<Route>{self.routes.iter().flatten().filter(|r|r.destination.contains(dst)).max_by_key(|r|(r.destination.prefix_len as u16, u16::MAX-r.metric)).copied()}
 pub fn len(&self)->usize{self.routes.iter().filter(|r|r.is_some()).count()}
}
