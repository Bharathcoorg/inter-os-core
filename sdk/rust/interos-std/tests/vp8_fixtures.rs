// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Lossy WebP (VP8 keyframe) decoder reference fixture tests.
//!
//! Provenance:
//! Fixtures in `tests/fixtures/vp8/` were generated with Google's `cwebp` (libwebp v1.3.2)
//! from source images at diverse quantization factors and dimensions. Each `.webp` fixture
//! is paired with byte-exact uncompressed RGBA pixel dumps produced by `dwebp -pam` / `dwebp`
//! (the canonical RFC 6386 / libwebp reference decoder). Fixture file format:
//! 4-byte little-endian width, 4-byte little-endian height, followed by raw RGBA byte buffer.
//!
//! Supported bitstream scope:
//! - VP8 intra keyframes (`key_frame == 0`), bitstream profile 0 (`version == 0`), `show_frame == 1`.
//! - RFC 6386 boolean arithmetic sub-decoder.
//! - 4-segment quantization with segment feature data and dequantization tables.
//! - All 4 luma 16x16 prediction modes (DC, V, H, TM).
//! - All 9 luma 4x4 intra prediction modes.
//! - All chroma 8x8 prediction modes.
//! - Normal and simple loop filters with macroblock boundary filtering.
//! - Fancy bilinear chroma upsampling.
//! - Bounded decode into static `.bss` arena ensuring `#![no_std]` zero-heap compliance.
//!
//! Explicitly refused features:
//! - Inter/delta frames (`key_frame != 0`) -> `Err(())`.
//! - Non-zero profiles/versions (`version != 0`) -> `Err(())`.
//! - VP8X extended containers with alpha (`flags & 0x10 != 0`) -> `Err(())`.
//! - VP8X extended containers with animation (`flags & 0x02 != 0`) -> `Err(())`.
//! - Dimensions exceeding 1024x1024 (`MAX_VP8_DIMENSION`) -> `Err(())`.
//! - Truncated headers (< 10 bytes) or invalid sync codes (`0x9D 0x01 0x2A`) -> `Err(())`.

use interos_std::image::{decode_webp, decode_webp_header};
use interos_std::vp8::{dimensions, decode};

#[test]
fn decodes_vp8_fixtures_or_checks_header() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vp8");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("webp"))
        .collect();
    entries.sort();

    let mut total_diffs = 0;
    let mut failures = Vec::new();
    for path in &entries {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let data = std::fs::read(path).unwrap();
        let expected = std::fs::read(path.with_extension("rgba")).unwrap();
        let width = u32::from_le_bytes(expected[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(expected[4..8].try_into().unwrap());
        let exp_pixels = &expected[8..];

        // Test decode_webp_header
        assert_eq!(decode_webp_header(&data), Ok((width, height)), "{path:?}");

        // Test decode_webp
        let mut out = vec![0u8; (width * height * 4) as usize];
        let res = decode_webp(&data, &mut out);
        assert!(res.is_ok(), "decode_webp failed for {path:?}: {res:?}");
        let (dec_w, dec_h, dec_len) = res.unwrap();
        assert_eq!(dec_w, width);
        assert_eq!(dec_h, height);
        assert_eq!(dec_len, out.len());

        let mut diff_count = 0;
        for (_i, (&actual, &exp)) in out.iter().zip(exp_pixels.iter()).enumerate() {
            if actual != exp {
                diff_count += 1;
            }
        }
        eprintln!(
            "Fixture {:<12} {:>3}x{:<3} {:>5} / {:>5} differing bytes",
            name, width, height, diff_count, exp_pixels.len()
        );
        if diff_count > 0 {
            failures.push(name.to_string());
        }
        total_diffs += diff_count;
    }
    assert!(entries.len() >= 5, "Expected at least 5 fixtures, saw {}", entries.len());
    assert_eq!(failures, Vec::<String>::new(), "Pixel mismatch in fixtures: {failures:?}");
    assert_eq!(total_diffs, 0, "All fixture pixels must match exactly");
}

#[test]
fn refuses_malformed_vp8_inputs() {
    let mut out = [0u8; 64 * 64 * 4];

    // Truncated chunk (< 10 bytes)
    assert!(dimensions(&[0u8; 9]).is_err());
    assert!(decode(&[0u8; 9], &mut out).is_err());

    // Bad sync bytes
    let mut bad_sync = [0u8; 16];
    bad_sync[0] = 0; // keyframe
    bad_sync[3] = 0x9D;
    bad_sync[4] = 0x02; // bad
    bad_sync[5] = 0x2A;
    assert!(dimensions(&bad_sync).is_err());
    assert!(decode(&bad_sync, &mut out).is_err());

    // Non-keyframe (bit 0 = 1)
    let mut non_key = [0u8; 16];
    non_key[0] = 1;
    non_key[3] = 0x9D;
    non_key[4] = 0x01;
    non_key[5] = 0x2A;
    assert!(dimensions(&non_key).is_err());
    assert!(decode(&non_key, &mut out).is_err());

    // Non-zero version (bits 1..3 != 0)
    let mut bad_ver = [0u8; 16];
    bad_ver[0] = 0x02; // version 1
    bad_ver[3] = 0x9D;
    bad_ver[4] = 0x01;
    bad_ver[5] = 0x2A;
    assert!(dimensions(&bad_ver).is_err());
    assert!(decode(&bad_ver, &mut out).is_err());

    // Oversized dimension (> 1024)
    let mut big_dim = [0u8; 16];
    big_dim[0] = 0x10; // show_frame = 1
    big_dim[3] = 0x9D;
    big_dim[4] = 0x01;
    big_dim[5] = 0x2A;
    big_dim[6] = 0x01;
    big_dim[7] = 0x08; // width = 2049
    big_dim[8] = 0x40;
    big_dim[9] = 0x00; // height = 64
    assert!(dimensions(&big_dim).is_err());
    assert!(decode(&big_dim, &mut out).is_err());
}

#[test]
fn refuses_vp8x_with_alpha_or_animation() {
    let mut out = [0u8; 64 * 64 * 4];
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vp8/grey_q75.webp");
    let grey_data = std::fs::read(&path).unwrap();
    // In grey_q75.webp: RIFF header (12 bytes), "VP8 " (4 bytes), size (4 bytes), then payload
    assert_eq!(&grey_data[12..16], b"VP8 ");
    let vp8_payload = &grey_data[20..];

    fn make_riff_webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WEBP");
        for (kind, payload) in chunks {
            body.extend_from_slice(*kind);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            if payload.len() & 1 != 0 {
                body.push(0);
            }
        }
        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
        riff.extend_from_slice(&body);
        riff
    }

    // Baseline: valid VP8 without VP8X decodes cleanly to (64, 64)
    let valid_stream = make_riff_webp(&[(b"VP8 ", vp8_payload)]);
    assert_eq!(decode_webp_header(&valid_stream), Ok((64, 64)));

    // Valid VP8X without alpha or animation (flags = 0) passes
    let vp8x_clean = [0u8; 10];
    let valid_vp8x = make_riff_webp(&[(b"VP8X", &vp8x_clean), (b"VP8 ", vp8_payload)]);
    assert_eq!(decode_webp_header(&valid_vp8x), Ok((64, 64)));
    assert!(decode_webp(&valid_vp8x, &mut out).is_ok());

    // 1. Alpha flag set (bit 4: 0x10) must be rejected even with a valid VP8 payload
    let mut vp8x_alpha = [0u8; 10];
    vp8x_alpha[0] = 0x10;
    let webp_alpha = make_riff_webp(&[(b"VP8X", &vp8x_alpha), (b"VP8 ", vp8_payload)]);
    assert!(decode_webp_header(&webp_alpha).is_err(), "VP8X with alpha must be refused");
    assert!(decode_webp(&webp_alpha, &mut out).is_err(), "VP8X with alpha must be refused");

    // 2. Animation flag set (bit 1: 0x02) must be rejected even with a valid VP8 payload
    let mut vp8x_anim = [0u8; 10];
    vp8x_anim[0] = 0x02;
    let webp_anim = make_riff_webp(&[(b"VP8X", &vp8x_anim), (b"VP8 ", vp8_payload)]);
    assert!(decode_webp_header(&webp_anim).is_err(), "VP8X with animation must be refused");
    assert!(decode_webp(&webp_anim, &mut out).is_err(), "VP8X with animation must be refused");
}

#[test]
fn fuzz_byte_truncation_never_panics() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vp8/grey_q75.webp");
    let data = std::fs::read(&path).unwrap();

    let mut out = [0u8; 64 * 64 * 4];
    for cut in 0..data.len() {
        let sub = &data[..cut];
        let _ = decode_webp_header(sub);
        let _ = decode_webp(sub, &mut out);
    }
}
