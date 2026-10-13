//! Canonical outline text engine shared from `interos_std::design::outline_text`.
//!
//! Provides the shared outline text rasterization engine over the SDK's typeface
//! without duplicating the rasterizer implementation across application binaries.

pub use interos_std::design::outline_text::*;

pub fn drawn_advance(text: &[u8], scale: usize) -> Option<usize> {
    interos_std::design::outline_text::drawn_advance(text, scale, None)
}
