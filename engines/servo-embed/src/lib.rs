//! Servo Embedding & Webview Interface for InterOS Core
//!
//! Under the InterOS Platform Truth Contract, this crate exposes an honest,
//! capability-checked interface for hosting Servo browser engine frames.
//! When the engine is not initialized or unsupported on the running hardware profile,
//! it reports `EngineStatus::Unavailable(reason)` rather than painting synthetic frames.

#![no_std]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatus {
    Ready,
    Loading,
    Unavailable(&'static str),
    NotImplemented,
}

pub struct ServoEmbed {
    status: EngineStatus,
    surface: Option<u32>,
    pub viewport_w: u32,
    pub viewport_h: u32,
}

impl ServoEmbed {
    pub const fn new(viewport_w: u32, viewport_h: u32) -> Self {
        Self {
            status: EngineStatus::Unavailable("Servo runtime requires x86_64-unknown-interos std target"),
            surface: None,
            viewport_w,
            viewport_h,
        }
    }

    pub fn status(&self) -> EngineStatus {
        self.status
    }

    pub fn attach_surface(&mut self, surface_id: u32) {
        self.surface = Some(surface_id);
    }

    pub fn surface(&self) -> Option<u32> {
        self.surface
    }

    pub fn load_url(&mut self, _url: &str) -> Result<(), &'static str> {
        Err("Servo engine runtime unavailable: native flow layout engine active")
    }

    pub fn inject_touch(&mut self, _x: u32, _y: u32, _kind: u8) {
        // Honest no-op when engine is unavailable
    }
}
