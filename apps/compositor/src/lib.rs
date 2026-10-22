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
}
