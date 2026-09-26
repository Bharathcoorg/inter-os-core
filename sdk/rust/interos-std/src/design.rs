//! InterOS Core visual design tokens shared by native Rust applications.
//!
//! These are semantic tokens, not app-specific styling. Apps should compose
//! surfaces, spacing and typography from this module so the launcher, system
//! line and native applications retain one visual language.

pub mod color {
    pub const BACKGROUND:u32=0xFF070B12;
    pub const SURFACE:u32=0xFF111B2B;
    pub const SURFACE_RAISED:u32=0xFF17243A;
    pub const BORDER:u32=0xFF26354D;
    pub const TEXT:u32=0xFFF4F7FB;
    pub const TEXT_MUTED:u32=0xFF9AA8BC;
    pub const ACCENT:u32=0xFF31C4FF;
    pub const DANGER:u32=0xFFFF6B6B;
    pub const WARNING:u32=0xFFFFC043;
    pub const SUCCESS:u32=0xFF56D68A;
}
pub mod spacing { pub const XS:usize=8; pub const SM:usize=12; pub const MD:usize=20; pub const LG:usize=28; pub const XL:usize=40; }
pub mod radius { pub const SM:u32=10; pub const MD:u32=16; pub const LG:u32=24; }
pub mod motion { pub const FAST:u32=120; pub const STANDARD:u32=220; pub const EMPHASIZED:u32=360; }
