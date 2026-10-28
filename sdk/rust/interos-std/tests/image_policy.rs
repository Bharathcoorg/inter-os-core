//! The platform's image policy, exercised through the public API.
//!
//! These are the three rules the platform holds every picture to: a size
//! cap that is checked before anything is sized from a file, a rule that
//! nothing outside a document is ever fetched, and a reader that fails
//! closed rather than drawing half a picture.

use interos_std::image::{
    check_decode_size, decode_bmp_header, decode_image, decode_ppm_header, decode_qoi_header,
    MAX_DECODE_DIMENSION, MAX_DECODE_PIXELS, MAX_ENCODED_BYTES,
};

/// A QOI header that claims a picture of the given size.
fn qoi_header(width: u32, height: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"qoif");
    body.extend_from_slice(&width.to_be_bytes());
    body.extend_from_slice(&height.to_be_bytes());
    body.push(4); // channels
    body.push(0); // colourspace
    body
}

/// A BMP header that claims a picture of the given size.
fn bmp_header(width: u32, height: u32) -> Vec<u8> {
    let mut body = vec![0u8; 54];
    body[0] = b'B';
    body[1] = b'M';
    body[10..14].copy_from_slice(&54u32.to_le_bytes()); // pixel data offset
    body[14..18].copy_from_slice(&40u32.to_le_bytes()); // header size
    body[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    body[22..26].copy_from_slice(&(height as i32).to_le_bytes());
    body[26..28].copy_from_slice(&1u16.to_le_bytes()); // planes
    body[28..30].copy_from_slice(&24u16.to_le_bytes()); // bits per pixel
    body[30..34].copy_from_slice(&0u32.to_le_bytes()); // compression: none
    body
}

#[test]
fn a_body_over_the_encoded_budget_is_refused_before_anything_is_read() {
    // A body over the budget is refused whole. It is never truncated into a
    // half-picture, because half a picture is not the file.
    let body = vec![0u8; MAX_ENCODED_BYTES + 1];
    let mut out = vec![0xAAu8; 64];
    assert!(decode_image(&body, b"x.png", &mut out).is_err());
    // Nothing was decoded into the caller's buffer.
    assert!(
        out.iter().all(|&byte| byte == 0xAA),
        "a refused body must not be decoded into the caller's buffer"
    );

    // The same rule at the picture's own entry point.
    assert!(interos_std::image::decode_svg_header(&body).is_err());
}

#[test]
fn a_hostile_length_field_is_refused_at_the_header() {
    // A fourteen-byte QOI header naming a picture of four billion pixels is
    // refused when the header is read, which is before a caller sizes a
    // buffer from it.
    assert!(decode_qoi_header(&qoi_header(u32::MAX, u32::MAX)).is_err());
    assert!(decode_qoi_header(&qoi_header(0xFFFF, 0xFFFF)).is_err());

    // The same for the other formats whose headers carry the size.
    assert!(decode_bmp_header(&bmp_header(100_000, 100_000)).is_err());

    let ppm = b"P6\n4294967295 4294967295\n255\n";
    assert!(decode_ppm_header(ppm).is_err());

    // A picture inside the budget is still admitted, so the cap is a cap
    // and not a refusal of everything.
    assert_eq!(
        decode_qoi_header(&qoi_header(64, 64)),
        Ok((64, 64, 4, 0))
    );
    assert_eq!(decode_ppm_header(b"P6\n2 2\n255\n").map(|(w, h, _)| (w, h)), Ok((2, 2)));
}

#[test]
fn the_decoded_size_has_one_explicit_budget() {
    // A side may be as long as the platform allows, and the picture as a
    // whole may not be bigger than the pixel budget.
    assert!(check_decode_size(MAX_DECODE_DIMENSION, 1).is_ok());
    assert!(check_decode_size(MAX_DECODE_DIMENSION + 1, 1).is_err());
    assert!(check_decode_size(1, MAX_DECODE_DIMENSION + 1).is_err());
    assert!(check_decode_size(0, 1).is_err());
    assert!(check_decode_size(1, 0).is_err());

    let side_pixels = (MAX_DECODE_DIMENSION as usize) * (MAX_DECODE_DIMENSION as usize);
    assert!(side_pixels > MAX_DECODE_PIXELS, "the pixel budget must be the tighter one");
    assert!(check_decode_size(MAX_DECODE_DIMENSION, MAX_DECODE_DIMENSION).is_err());
    assert!(check_decode_size(2048, 2048).is_ok());
}

#[test]
fn a_vector_document_that_names_a_remote_resource_is_refused() {
    let mut out = vec![0u8; 64 * 64 * 4];

    // Nothing outside the document is ever fetched, so a document that asks
    // for something outside it is refused rather than drawn without it.
    let remote = br#"<svg width="64" height="64"><image href="http://example.com/x.png"/></svg>"#;
    assert!(decode_image(remote, b"x.svg", &mut out).is_err());

    let remote_fill = br#"<svg width="64" height="64"><rect width="64" height="64" fill="url(http://example.com/x.svg#g)"/></svg>"#;
    assert!(decode_image(remote_fill, b"x.svg", &mut out).is_err());

    // A paint server this reader does not implement is refused as well:
    // drawing a substitute would be inventing a picture.
    let gradient = br##"<svg width="64" height="64"><defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/></svg>"##;
    assert!(decode_image(gradient, b"x.svg", &mut out).is_err());

    // A document this reader can draw is still drawn.
    let plain = br##"<svg width="64" height="64"><rect width="64" height="64" fill="#ff0000"/></svg>"##;
    assert!(decode_image(plain, b"x.svg", &mut out).is_ok());
}
