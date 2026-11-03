// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]

pub use interos_std as std;

pub const W: u32 = 720;
pub const H: u32 = 96;
pub const CHUNK: usize = 1024;
pub const BAR_PERIOD: u64 = 600;
pub const TOTAL_PIXELS: usize = (W * H) as usize;
pub const NUM_CHUNKS: usize = (TOTAL_PIXELS + CHUNK - 1) / CHUNK;

/// Native 1080x2160 Full HD scanout dimensions and tiling geometry
pub const FHD_W: u32 = 1080;
pub const FHD_H: u32 = 2160;
pub const FHD_TOTAL_PIXELS: usize = (FHD_W * FHD_H) as usize;
pub const FHD_NUM_CHUNKS: usize = (FHD_TOTAL_PIXELS + CHUNK - 1) / CHUNK;

/// Computes the offset and pixel count of a chunk by index.
pub fn chunk_bounds(chunk_idx: usize) -> Option<(usize, usize)> {
    if chunk_idx >= NUM_CHUNKS {
        return None;
    }
    let offset = chunk_idx * CHUNK;
    let count = (TOTAL_PIXELS - offset).min(CHUNK);
    Some((offset, count))
}

/// Computes the offset and pixel count of a chunk by index for a full-resolution 1080x2160 surface.
pub fn chunk_bounds_fhd(chunk_idx: usize) -> Option<(usize, usize)> {
    if chunk_idx >= FHD_NUM_CHUNKS {
        return None;
    }
    let offset = chunk_idx * CHUNK;
    let count = (FHD_TOTAL_PIXELS - offset).min(CHUNK);
    Some((offset, count))
}

/// Computes the cyclic sweep phase of the progress bar in ticks.
pub fn sweep_phase(ticks: u64) -> u64 {
    ticks % BAR_PERIOD
}

/// Builds the composite cache key for change detection.
pub fn cache_key(ticks: u64, windows: usize, services: usize) -> (u64, usize, usize) {
    (sweep_phase(ticks), windows, services)
}

/// Builds the declarative DisplayList commands for the system-line overlay.
///
/// Records base card fill, activity progress squircle, and status indicators
/// with zero heap allocations.
pub fn render_system_line_display_list(ticks: u64, windows: usize, services: usize) -> interos_std::design::display_list::DisplayList<16> {
    use interos_std::design::display_list::{DisplayList, Rect};
    let mut dl = DisplayList::<16>::new(0xFF000000); // Platform system-line black backdrop

    // 1. Base system line fill
    dl.push_fill_rect(Rect::new(0, 0, W as usize, H as usize), 0xFF000000);

    // 2. Activity sweep progress indicator squircle
    let phase = sweep_phase(ticks) as usize;
    let sweep_x = (phase * (W as usize - 100)) / (BAR_PERIOD as usize);
    let sweep_w = 80;
    let sweep_h = 4;
    let sweep_y = (H as usize).saturating_sub(sweep_h + 4);
    dl.push_squircle(Rect::new(sweep_x, sweep_y, sweep_w, sweep_h), 0xFF4080FF, 2, None);

    // 3. Status indicator pills for active windows & services
    if windows > 0 {
        dl.push_fill_rect(Rect::new(16, 8, 8, 8), 0xFF00E080); // Active window green indicator
    }
    if services > 0 {
        dl.push_fill_rect(Rect::new(32, 8, 8, 8), 0xFF40A0FF); // Active service blue indicator
    }

    dl
}

/// Decodes and validates an incoming declarative DisplayList frame from an SHM pipe.
///
/// Returns the bounding damage rectangle that requires direct scanout rasterization.
pub fn process_shm_display_list(rx_data: &[u8]) -> Result<interos_std::design::display_list::Rect, &'static str> {
    use interos_std::design::display_list::DisplayList;
    let dl = DisplayList::<32>::decode_from_slice(rx_data)?;
    dl.dirty_rect().ok_or("no damage rectangle in display list")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_line_geometry() {
        assert_eq!(TOTAL_PIXELS, 69120);
        assert_eq!(NUM_CHUNKS, 68);

        // Verify all chunks seamlessly tile 0..TOTAL_PIXELS
        let mut total_covered = 0;
        for i in 0..NUM_CHUNKS {
            let (offset, count) = chunk_bounds(i).expect("chunk bounds");
            assert_eq!(offset, total_covered);
            assert!(count <= CHUNK);
            total_covered += count;
        }
        assert_eq!(total_covered, TOTAL_PIXELS);

        // Out of bounds
        assert_eq!(chunk_bounds(NUM_CHUNKS), None);
    }

    #[test]
    fn test_last_chunk_remainder() {
        let (offset, count) = chunk_bounds(67).expect("last chunk");
        assert_eq!(offset, 67 * 1024); // 68608
        assert_eq!(count, 512); // 69120 - 68608 = 512
    }

    #[test]
    fn test_sweep_phase() {
        assert_eq!(sweep_phase(0), 0);
        assert_eq!(sweep_phase(300), 300);
        assert_eq!(sweep_phase(600), 0);
        assert_eq!(sweep_phase(1205), 5);

        assert_eq!(cache_key(600, 1, 2), (0, 1, 2));
    }

    #[test]
    fn test_fhd_scanout_geometry() {
        assert_eq!(FHD_TOTAL_PIXELS, 1080 * 2160);
        assert_eq!(FHD_NUM_CHUNKS, 2279); // (2332800 + 1023) / 1024

        let mut total_covered = 0;
        for i in 0..FHD_NUM_CHUNKS {
            let (offset, count) = chunk_bounds_fhd(i).expect("fhd chunk bounds");
            assert_eq!(offset, total_covered);
            assert!(count <= CHUNK);
            total_covered += count;
        }
        assert_eq!(total_covered, FHD_TOTAL_PIXELS);
        assert_eq!(chunk_bounds_fhd(FHD_NUM_CHUNKS), None);

        // Remainder chunk check
        let (last_offset, last_count) = chunk_bounds_fhd(FHD_NUM_CHUNKS - 1).expect("last fhd chunk");
        assert_eq!(last_offset, 2278 * 1024); // 2,332,672
        assert_eq!(last_count, 128); // 2332800 - 2332672 = 128
    }

    #[test]
    fn test_system_line_display_list_recording_and_rasterization() {
        let dl = render_system_line_display_list(300, 1, 1);
        assert_eq!(dl.len(), 4); // base fill + progress squircle + window dot + service dot

        let damage = dl.dirty_rect().expect("damage rect");
        assert_eq!(damage.w, W as usize);
        assert_eq!(damage.h, H as usize);

        let mut band = [0u32; (W * 10) as usize];
        dl.rasterize_band(0, 10, W as usize, &mut band);
        // Verify green indicator dot rendered at (16, 8)
        assert_eq!(band[8 * (W as usize) + 16], 0xFF00E080);
    }

    #[test]
    fn test_shm_pipe_display_list_processing() {
        use interos_std::design::display_list::{DisplayList, Rect};
        let mut client_dl = DisplayList::<8>::new(0xFF000000);
        client_dl.push_fill_rect(Rect::new(50, 10, 200, 30), 0xFFFFFFFF);

        let mut wire_buf = [0u8; 256];
        let bytes_written = client_dl.encode_to_slice(&mut wire_buf).expect("encode client dl");

        let damage = process_shm_display_list(&wire_buf[..bytes_written]).expect("process shm dl");
        assert_eq!(damage.x, 50);
        assert_eq!(damage.y, 10);
        assert_eq!(damage.w, 200);
        assert_eq!(damage.h, 30);
    }
}
