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
}
