// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

use interos_std::image::{decode_webp, decode_webp_header};

/// Every fixture is a lossless WebP produced by libwebp's encoder plus the
/// RGBA bytes that libwebp's decoder returns for it: a little-endian width,
/// a little-endian height, then four bytes per pixel.
#[test]
fn decodes_libwebp_encoder_output_exactly() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webp");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("webp") {
            continue;
        }
        seen += 1;
        let data = std::fs::read(&path).unwrap();
        let expected = std::fs::read(path.with_extension("rgba")).unwrap();
        let width = u32::from_le_bytes(expected[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(expected[4..8].try_into().unwrap());
        assert_eq!(decode_webp_header(&data), Ok((width, height)), "{path:?}");
        let mut out = vec![0u8; (width * height * 4) as usize];
        assert_eq!(
            decode_webp(&data, &mut out),
            Ok((width, height, out.len())),
            "{path:?}"
        );
        assert_eq!(out, expected[8..], "{path:?}");
    }
    assert_eq!(seen, 6);
}
