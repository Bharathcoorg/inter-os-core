#![no_std]

pub use interos_std as std;

/// Operating mode of the Sovereign Camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Photo = 0,
    Video = 1,
    DocumentVision = 2,
    QrScanner = 3,
    NightMode = 4,
}

/// Flash torch hardware control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    Off = 0,
    On = 1,
    Auto = 2,
    Torch = 3,
}

/// Real-time ISP color and tonal filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFilter {
    Normal = 0,
    Monochrome = 1,
    HighContrast = 2,
    Sepia = 3,
    NeuralHeatmap = 4,
}

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
pub fn fmt_u32(mut v: u32, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    while v > 0 && n < 10 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Applies software ISP tone curve adjustments directly on raw RGBA pixel buffer.
pub fn apply_isp_tone_curve(filter: CameraFilter, exposure_bias: i16, buf: &mut [u8]) {
    for pixel in buf.chunks_exact_mut(4) {
        let r = (pixel[0] as i16 + exposure_bias * 16).clamp(0, 255) as u8;
        let g = (pixel[1] as i16 + exposure_bias * 16).clamp(0, 255) as u8;
        let b = (pixel[2] as i16 + exposure_bias * 16).clamp(0, 255) as u8;

        match filter {
            CameraFilter::Normal => {
                pixel[0] = r;
                pixel[1] = g;
                pixel[2] = b;
            }
            CameraFilter::Monochrome => {
                let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
                pixel[0] = luma;
                pixel[1] = luma;
                pixel[2] = luma;
            }
            CameraFilter::HighContrast => {
                let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
                let high = if luma > 128 { 255 } else { 0 };
                pixel[0] = high;
                pixel[1] = high;
                pixel[2] = high;
            }
            CameraFilter::Sepia => {
                pixel[0] = ((r as u32 * 393 + g as u32 * 769 + b as u32 * 189) / 1000).min(255) as u8;
                pixel[1] = ((r as u32 * 349 + g as u32 * 686 + b as u32 * 168) / 1000).min(255) as u8;
                pixel[2] = ((r as u32 * 272 + g as u32 * 534 + b as u32 * 131) / 1000).min(255) as u8;
            }
            CameraFilter::NeuralHeatmap => {
                pixel[0] = 255 - r;
                pixel[1] = g;
                pixel[2] = b;
            }
        }
    }
}

/// Computes full-frame RGB luminance metrics.
pub fn compute_histogram(buf: &[u8]) -> (u8, u8, u8) {
    let mut r_acc: u64 = 0;
    let mut g_acc: u64 = 0;
    let mut b_acc: u64 = 0;
    let mut count: u64 = 0;

    for pixel in buf.chunks_exact(4) {
        r_acc += pixel[0] as u64;
        g_acc += pixel[1] as u64;
        b_acc += pixel[2] as u64;
        count += 1;
    }

    if count == 0 {
        return (0, 0, 0);
    }

    (
        (r_acc / count) as u8,
        (g_acc / count) as u8,
        (b_acc / count) as u8,
    )
}

/// Hashes an RGBA color tuple into the 64-entry running color cache.
#[inline(always)]
pub fn qoi_color_hash(r: u8, g: u8, b: u8, a: u8) -> usize {
    (r as usize * 3 + g as usize * 5 + b as usize * 7 + a as usize * 11) % 64
}

/// Zero-allocation, streaming QOI image encoder.
///
/// Encodes raw RGBA (32-bit, 4 bytes per pixel) into Quite OK Image (QOI)
/// format directly into `out`. Returns the number of bytes written to `out`,
/// or `Err(())` if `out` lacks sufficient capacity or input length is invalid.
pub fn encode_qoi(width: u32, height: u32, rgba_pixels: &[u8], out: &mut [u8]) -> Result<usize, ()> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .ok_or(())?
        .checked_mul(4)
        .ok_or(())?;
    if rgba_pixels.len() < expected_len {
        return Err(());
    }

    if out.len() < 22 {
        return Err(());
    }

    // 14-byte QOI Header
    out[0..4].copy_from_slice(b"qoif");
    out[4..8].copy_from_slice(&width.to_be_bytes());
    out[8..12].copy_from_slice(&height.to_be_bytes());
    out[12] = 4; // RGBA channels
    out[13] = 0; // sRGB colorspace

    let mut out_pos = 14usize;
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0u8, 0u8, 255u8];
    let mut run = 0u8;

    for chunk in rgba_pixels[..expected_len].chunks_exact(4) {
        let px = [chunk[0], chunk[1], chunk[2], chunk[3]];

        if px == prev {
            run += 1;
            if run == 62 {
                if out_pos >= out.len() {
                    return Err(());
                }
                out[out_pos] = 0b1100_0000 | (run - 1);
                out_pos += 1;
                run = 0;
            }
        } else {
            if run > 0 {
                if out_pos >= out.len() {
                    return Err(());
                }
                out[out_pos] = 0b1100_0000 | (run - 1);
                out_pos += 1;
                run = 0;
            }

            let idx = qoi_color_hash(px[0], px[1], px[2], px[3]);
            if index[idx] == px {
                if out_pos >= out.len() {
                    return Err(());
                }
                out[out_pos] = 0b0000_0000 | (idx as u8);
                out_pos += 1;
            } else {
                index[idx] = px;

                if px[3] == prev[3] {
                    let vr = px[0].wrapping_sub(prev[0]) as i8;
                    let vg = px[1].wrapping_sub(prev[1]) as i8;
                    let vb = px[2].wrapping_sub(prev[2]) as i8;

                    let vg_r = vr.wrapping_sub(vg);
                    let vg_b = vb.wrapping_sub(vg);

                    if vr >= -2 && vr <= 1 && vg >= -2 && vg <= 1 && vb >= -2 && vb <= 1 {
                        if out_pos >= out.len() {
                            return Err(());
                        }
                        out[out_pos] = 0b0100_0000
                            | (((vr + 2) as u8) << 4)
                            | (((vg + 2) as u8) << 2)
                            | ((vb + 2) as u8);
                        out_pos += 1;
                    } else if vg >= -32 && vg <= 31 && vg_r >= -8 && vg_r <= 7 && vg_b >= -8 && vg_b <= 7 {
                        if out_pos + 2 > out.len() {
                            return Err(());
                        }
                        out[out_pos] = 0b1000_0000 | ((vg + 32) as u8);
                        out[out_pos + 1] = (((vg_r + 8) as u8) << 4) | ((vg_b + 8) as u8);
                        out_pos += 2;
                    } else {
                        if out_pos + 4 > out.len() {
                            return Err(());
                        }
                        out[out_pos] = 0xFE;
                        out[out_pos + 1] = px[0];
                        out[out_pos + 2] = px[1];
                        out[out_pos + 3] = px[2];
                        out_pos += 4;
                    }
                } else {
                    if out_pos + 5 > out.len() {
                        return Err(());
                    }
                    out[out_pos] = 0xFF;
                    out[out_pos + 1] = px[0];
                    out[out_pos + 2] = px[1];
                    out[out_pos + 3] = px[2];
                    out[out_pos + 4] = px[3];
                    out_pos += 5;
                }
            }
            prev = px;
        }
    }

    if run > 0 {
        if out_pos >= out.len() {
            return Err(());
        }
        out[out_pos] = 0b1100_0000 | (run - 1);
        out_pos += 1;
    }

    if out_pos + 8 > out.len() {
        return Err(());
    }
    out[out_pos..out_pos + 8].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out_pos += 8;

    Ok(out_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_isp_monochrome_filter() {
        // Red pixel: (255, 0, 0, 255)
        let mut pixels = [255u8, 0, 0, 255];
        apply_isp_tone_curve(CameraFilter::Monochrome, 0, &mut pixels);
        // (255 * 299) / 1000 = 76
        assert_eq!(pixels[0], 76);
        assert_eq!(pixels[1], 76);
        assert_eq!(pixels[2], 76);
    }

    #[test]
    fn test_isp_high_contrast_filter() {
        let mut dark = [50u8, 50, 50, 255];
        apply_isp_tone_curve(CameraFilter::HighContrast, 0, &mut dark);
        assert_eq!(dark[0], 0);

        let mut bright = [200u8, 200, 200, 255];
        apply_isp_tone_curve(CameraFilter::HighContrast, 0, &mut bright);
        assert_eq!(bright[0], 255);
    }

    #[test]
    fn test_compute_histogram() {
        let pixels = [
            100u8, 150, 200, 255,
            200, 50, 100, 255,
        ];
        let (r, g, b) = compute_histogram(&pixels);
        assert_eq!(r, 150);
        assert_eq!(g, 100);
        assert_eq!(b, 150);
    }

    #[test]
    fn test_fmt_u32() {
        let mut buf = [0u8; 16];
        let n = fmt_u32(1920, &mut buf);
        assert_eq!(&buf[..n], b"1920");

        let n0 = fmt_u32(0, &mut buf);
        assert_eq!(&buf[..n0], b"0");
    }

    #[test]
    fn test_qoi_encode_solid_color() {
        let pixels = [255u8, 0, 0, 255].repeat(16); // 4x4 red
        let mut out = [0u8; 128];
        let bytes_written = encode_qoi(4, 4, &pixels, &mut out).expect("QOI encode failed");
        assert!(bytes_written >= 22);
        assert_eq!(&out[0..4], b"qoif");
        assert_eq!(&out[4..8], &4u32.to_be_bytes());
        assert_eq!(&out[8..12], &4u32.to_be_bytes());
        assert_eq!(out[12], 4); // RGBA
        assert_eq!(&out[bytes_written - 8..bytes_written], &[0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn test_qoi_encode_capacity_overflow() {
        let pixels = [0u8; 16];
        let mut out = [0u8; 10]; // Smaller than 22-byte minimum
        assert_eq!(encode_qoi(2, 2, &pixels, &mut out), Err(()));
    }
}

