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
