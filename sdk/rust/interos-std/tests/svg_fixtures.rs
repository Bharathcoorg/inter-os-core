#[path = "../src/image/policy.rs"]
mod policy;

#[path = "../src/image/svg.rs"]
mod svg;

use svg::*;

#[test]
fn test_decode_svg_header_dimensions() {
    let svg1 = br#"<svg width="120" height="80"></svg>"#;
    assert_eq!(decode_svg_header(svg1), Ok((120, 80)));

    let svg_px = br#"<svg width="64px" height="48px"></svg>"#;
    assert_eq!(decode_svg_header(svg_px), Ok((64, 48)));

    let svg_vb = br#"<svg viewBox="0 0 240 160"></svg>"#;
    assert_eq!(decode_svg_header(svg_vb), Ok((240, 160)));

    let svg_both = br#"<svg width="100" height="100" viewBox="0 0 50 50"></svg>"#;
    assert_eq!(decode_svg_header(svg_both), Ok((100, 100)));

    let svg_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Sample comment -->
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg width="200" height="150" viewBox="0 0 100 75">
</svg>"#;
    assert_eq!(decode_svg_header(svg_xml), Ok((200, 150)));
}

#[test]
fn test_decode_svg_header_malformed() {
    // Empty data
    assert_eq!(decode_svg_header(b""), Err(()));

    // Non-svg xml
    assert_eq!(decode_svg_header(b"<html><body>test</body></html>"), Err(()));

    // Zero dimensions
    assert_eq!(decode_svg_header(br#"<svg width="0" height="100"></svg>"#), Err(()));
    assert_eq!(decode_svg_header(br#"<svg viewBox="0 0 0 0"></svg>"#), Err(()));

    // Missing dimensions and viewBox
    assert_eq!(decode_svg_header(br#"<svg id="mysvg"></svg>"#), Err(()));

    // Unclosed svg tag
    assert_eq!(decode_svg_header(br#"<svg width="100" height="100""#), Err(()));
}

#[test]
fn test_render_rect_fill_and_stroke() {
    let svg = br#"<svg width="20" height="20">
        <rect x="4" y="4" width="12" height="12" fill="red"/>
    </svg>"#;
    let mut out = [0u8; 20 * 20 * 4];
    let (w, h, len) = decode_svg(svg, &mut out).expect("decode failed");
    assert_eq!((w, h), (20, 20));
    assert_eq!(len, 20 * 20 * 4);

    // Center pixel (10, 10) must be red (255, 0, 0, 255)
    let idx = (10 * 20 + 10) * 4;
    assert_eq!(&out[idx..idx + 4], &[255, 0, 0, 255]);

    // Outside pixel (0, 0) must be transparent
    assert_eq!(&out[0..4], &[0, 0, 0, 0]);

    // Stroke-only rect
    let stroke_svg = br#"<svg width="20" height="20">
        <rect x="2" y="2" width="16" height="16" fill="none" stroke="blue" stroke-width="2"/>
    </svg>"#;
    let mut out2 = [0u8; 20 * 20 * 4];
    decode_svg(stroke_svg, &mut out2).expect("stroke decode failed");

    // Center (10, 10) must remain transparent
    let center_idx = (10 * 20 + 10) * 4;
    assert_eq!(&out2[center_idx..center_idx + 4], &[0, 0, 0, 0]);

    // Border pixel (2, 2) must have blue
    let border_idx = (2 * 20 + 2) * 4;
    assert!(out2[border_idx + 2] > 200, "Blue channel expected on border");
    assert!(out2[border_idx + 3] > 200, "Alpha channel expected on border");
}

#[test]
fn test_render_rounded_rect() {
    let svg = br#"<svg width="30" height="30">
        <rect x="2" y="2" width="26" height="26" rx="6" ry="6" fill="green"/>
    </svg>"#;
    let mut out = [0u8; 30 * 30 * 4];
    decode_svg(svg, &mut out).expect("rounded rect decode failed");

    // Center (15, 15) must be green (0, 128, 0, 255)
    let center_idx = (15 * 30 + 15) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[0, 128, 0, 255]);

    // Corner (2, 2) is outside the rounded arc
    let corner_idx = (2 * 30 + 2) * 4;
    assert_eq!(&out[corner_idx..corner_idx + 4], &[0, 0, 0, 0]);

    // Top-middle (15, 3) must be filled
    let top_idx = (3 * 30 + 15) * 4;
    assert_eq!(&out[top_idx..top_idx + 4], &[0, 128, 0, 255]);
}

#[test]
fn test_render_circle_fill_and_stroke() {
    let svg = br#"<svg width="40" height="40">
        <circle cx="20" cy="20" r="10" fill="yellow"/>
    </svg>"#;
    let mut out = [0u8; 40 * 40 * 4];
    decode_svg(svg, &mut out).expect("circle decode failed");

    // Center (20, 20) must be yellow (255, 255, 0, 255)
    let center_idx = (20 * 40 + 20) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[255, 255, 0, 255]);

    // Far corner (0, 0) must be transparent
    assert_eq!(&out[0..4], &[0, 0, 0, 0]);

    // Point outside radius: (20, 5) is distance 15 > r=10 -> transparent
    let out_idx = (5 * 40 + 20) * 4;
    assert_eq!(&out[out_idx..out_idx + 4], &[0, 0, 0, 0]);

    // Circle stroke
    let stroke_svg = br#"<svg width="40" height="40">
        <circle cx="20" cy="20" r="12" fill="none" stroke="magenta" stroke-width="4"/>
    </svg>"#;
    let mut out2 = [0u8; 40 * 40 * 4];
    decode_svg(stroke_svg, &mut out2).expect("circle stroke decode failed");

    // Center (20, 20) must be transparent
    assert_eq!(&out2[center_idx..center_idx + 4], &[0, 0, 0, 0]);

    // On ring (20, 8) distance is 12 -> magenta (255, 0, 255)
    let ring_idx = (8 * 40 + 20) * 4;
    assert!(out2[ring_idx] > 200, "Red channel for magenta on ring");
    assert!(out2[ring_idx + 2] > 200, "Blue channel for magenta on ring");
}

#[test]
fn test_render_path_linear_and_relative() {
    let svg = br#"<svg width="30" height="30">
        <path d="M 5 5 H 25 V 25 H 5 Z" fill="white"/>
    </svg>"#;
    let mut out = [0u8; 30 * 30 * 4];
    decode_svg(svg, &mut out).expect("path decode failed");

    // Center (15, 15) must be white
    let center_idx = (15 * 30 + 15) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[255, 255, 255, 255]);

    // Outside (0, 0) must be transparent
    assert_eq!(&out[0..4], &[0, 0, 0, 0]);

    // Relative commands m, h, v, z
    let rel_svg = br#"<svg width="30" height="30">
        <path d="m 5 5 h 20 v 20 h -20 z" fill="cyan"/>
    </svg>"#;
    let mut out_rel = [0u8; 30 * 30 * 4];
    decode_svg(rel_svg, &mut out_rel).expect("relative path failed");
    assert_eq!(&out_rel[center_idx..center_idx + 4], &[0, 255, 255, 255]);
}

#[test]
fn test_render_path_bezier_curves() {
    // Cubic Bézier C
    let cubic_svg = br#"<svg width="40" height="40">
        <path d="M 5 35 C 5 5 35 5 35 35 Z" fill="red"/>
    </svg>"#;
    let mut out = [0u8; 40 * 40 * 4];
    decode_svg(cubic_svg, &mut out).expect("cubic bezier failed");
    let mid_idx = (25 * 40 + 20) * 4;
    assert_eq!(&out[mid_idx..mid_idx + 4], &[255, 0, 0, 255]);

    // Smooth cubic S
    let s_svg = br#"<svg width="50" height="50">
        <path d="M 5 25 C 10 10 20 10 25 25 S 40 40 45 25 Z" fill="blue"/>
    </svg>"#;
    let mut out_s = [0u8; 50 * 50 * 4];
    decode_svg(s_svg, &mut out_s).expect("smooth cubic bezier failed");
    let test_idx = (20 * 50 + 15) * 4;
    assert_eq!(out_s[test_idx + 2], 255);

    // Quadratic Bézier Q
    let quad_svg = br#"<svg width="40" height="40">
        <path d="M 5 35 Q 20 5 35 35 Z" fill="green"/>
    </svg>"#;
    let mut out_q = [0u8; 40 * 40 * 4];
    decode_svg(quad_svg, &mut out_q).expect("quad bezier failed");
    assert_eq!(&out_q[mid_idx..mid_idx + 4], &[0, 128, 0, 255]);

    // Smooth quadratic T
    let t_svg = br#"<svg width="50" height="50">
        <path d="M 5 25 Q 15 10 25 25 T 45 25 Z" fill="yellow"/>
    </svg>"#;
    let mut out_t = [0u8; 50 * 50 * 4];
    decode_svg(t_svg, &mut out_t).expect("smooth quad bezier failed");
}

#[test]
fn test_viewbox_scaling() {
    let svg = br#"<svg width="40" height="40" viewBox="0 0 20 20">
        <rect x="0" y="0" width="10" height="10" fill="red"/>
    </svg>"#;
    let mut out = [0u8; 40 * 40 * 4];
    decode_svg(svg, &mut out).expect("viewBox scaling failed");

    // The rect [0..10, 0..10] in 20x20 viewBox maps to [0..20, 0..20] on 40x40 canvas
    // Inside pixel (10, 10) must be red
    let in_idx = (10 * 40 + 10) * 4;
    assert_eq!(&out[in_idx..in_idx + 4], &[255, 0, 0, 255]);

    // Pixel at (18, 18) must also be red
    let edge_idx = (18 * 40 + 18) * 4;
    assert_eq!(&out[edge_idx..edge_idx + 4], &[255, 0, 0, 255]);

    // Outside pixel at (30, 30) must be transparent
    let out_idx = (30 * 40 + 30) * 4;
    assert_eq!(&out[out_idx..out_idx + 4], &[0, 0, 0, 0]);
}

#[test]
fn test_colors_and_styles() {
    let svg = br##"<svg width="20" height="20">
        <rect x="0" y="0" width="10" height="10" style="fill: #112233; stroke: none"/>
        <rect x="10" y="0" width="10" height="10" fill="#f00"/>
        <rect x="0" y="10" width="10" height="10" fill="currentColor"/>
        <rect x="10" y="10" width="10" height="10" fill="darkgray"/>
    </svg>"##;
    let mut out = [0u8; 20 * 20 * 4];
    decode_svg(svg, &mut out).expect("colors and styles failed");

    // (5, 5) -> #112233
    let p0 = (5 * 20 + 5) * 4;
    assert_eq!(&out[p0..p0 + 4], &[0x11, 0x22, 0x33, 255]);

    // (15, 5) -> #f00 -> (255, 0, 0, 255)
    let p1 = (5 * 20 + 15) * 4;
    assert_eq!(&out[p1..p1 + 4], &[255, 0, 0, 255]);

    // (5, 15) -> currentColor -> (0, 0, 0, 255)
    let p2 = (15 * 20 + 5) * 4;
    assert_eq!(&out[p2..p2 + 4], &[0, 0, 0, 255]);

    // (15, 15) -> darkgray -> (169, 169, 169, 255)
    let p3 = (15 * 20 + 15) * 4;
    assert_eq!(&out[p3..p3 + 4], &[169, 169, 169, 255]);
}

#[test]
fn test_group_style_inheritance() {
    let svg = br#"<svg width="20" height="20">
        <g fill="blue">
            <rect x="2" y="2" width="16" height="16"/>
        </g>
    </svg>"#;
    let mut out = [0u8; 20 * 20 * 4];
    decode_svg(svg, &mut out).expect("group inheritance failed");
    let p = (10 * 20 + 10) * 4;
    assert_eq!(&out[p..p + 4], &[0, 0, 255, 255]);
}

#[test]
fn test_malformed_svg_rejections() {
    // Corrupt path d string
    let bad_cmd = br#"<svg width="20" height="20"><path d="M 5 5 X 10 10"/></svg>"#;
    let mut out = [0u8; 20 * 20 * 4];
    assert_eq!(decode_svg(bad_cmd, &mut out), Err(()));

    // Truncated path coordinates
    let bad_coords = br#"<svg width="20" height="20"><path d="M 5"/></svg>"#;
    assert_eq!(decode_svg(bad_coords, &mut out), Err(()));

    // Destination buffer too small
    let valid_svg = br#"<svg width="20" height="20"><rect width="20" height="20"/></svg>"#;
    let mut small_buf = [0u8; 100];
    assert_eq!(decode_svg(valid_svg, &mut small_buf), Err(()));
}

#[test]
fn test_documented_bounds_limits() {
    // MAX_WIDTH bounds: 1280 is valid, 1281 is rejected
    assert_eq!(decode_svg_header(br#"<svg width="1280" height="20"></svg>"#), Ok((1280, 20)));
    assert_eq!(decode_svg_header(br#"<svg width="1281" height="20"></svg>"#), Err(()));

    // MAX_SEGMENTS bounds: pushing > 1024 segments returns Err(())
    let mut segs = SegmentBuffer::new();
    for i in 0..MAX_SEGMENTS {
        assert!(segs.push(0, 0, (i as i32) * ONE, (i as i32) * ONE).is_ok());
    }
    assert_eq!(segs.len, MAX_SEGMENTS);
    assert_eq!(segs.push(0, 0, 100, 100), Err(()));

    // MAX_CROSSINGS bounds: rasterizer handles dense segments safely without panic
    let mut rasterizer = SvgRasterizer::new();
    let mut out = [0u8; 100 * 10 * 4];
    rasterizer.rasterize(&segs, 100, 10, (255, 0, 0, 255), FillRule::NonZero, &mut out);
}

#[test]
fn test_all_named_colors() {
    let colors = [
        ("black", (0, 0, 0, 255)),
        ("white", (255, 255, 255, 255)),
        ("red", (255, 0, 0, 255)),
        ("green", (0, 128, 0, 255)),
        ("blue", (0, 0, 255, 255)),
        ("yellow", (255, 255, 0, 255)),
        ("cyan", (0, 255, 255, 255)),
        ("magenta", (255, 0, 255, 255)),
        ("gray", (128, 128, 128, 255)),
        ("lightgray", (211, 211, 211, 255)),
        ("darkgray", (169, 169, 169, 255)),
    ];

    for (name, expected) in colors {
        let parsed = parse_color(name.as_bytes()).unwrap_or_else(|| panic!("failed to parse {}", name));
        assert_eq!(parsed, Color::Rgba(expected.0, expected.1, expected.2, expected.3), "Mismatch for {name}");
    }

    assert_eq!(parse_color(b"none"), Some(Color::None));
    assert_eq!(parse_color(b"currentColor"), Some(Color::Rgba(0, 0, 0, 255)));
}

#[test]
fn test_implicit_lineto_after_moveto() {
    let svg = br#"<svg width="30" height="30">
        <path d="M 5 5 25 5 25 25 5 25 Z" fill="red"/>
    </svg>"#;
    let mut out = [0u8; 30 * 30 * 4];
    decode_svg(svg, &mut out).expect("implicit lineto decode failed");
    let center_idx = (15 * 30 + 15) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[255, 0, 0, 255]);

    // Relative implicit lineto
    let rel_svg = br#"<svg width="30" height="30">
        <path d="m 5 5 20 0 0 20 -20 0 z" fill="blue"/>
    </svg>"#;
    let mut out_rel = [0u8; 30 * 30 * 4];
    decode_svg(rel_svg, &mut out_rel).expect("relative implicit lineto failed");
    assert_eq!(&out_rel[center_idx..center_idx + 4], &[0, 0, 255, 255]);
}

#[test]
fn test_negative_viewbox_offset() {
    let svg = br#"<svg width="40" height="40" viewBox="-20 -20 40 40">
        <rect x="-10" y="-10" width="20" height="20" fill="green"/>
    </svg>"#;
    let mut out = [0u8; 40 * 40 * 4];
    decode_svg(svg, &mut out).expect("negative viewBox failed");

    // Center of viewBox is (0, 0), which maps to canvas (20, 20)
    let center_idx = (20 * 40 + 20) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[0, 128, 0, 255]);

    // Corner of canvas (0, 0) corresponds to (-20, -20) in viewBox, outside [-10..10]
    assert_eq!(&out[0..4], &[0, 0, 0, 0]);
}

#[test]
fn test_path_stroking() {
    let svg = br#"<svg width="40" height="40">
        <path d="M 5 5 L 35 5 L 35 35" stroke="red" stroke-width="4" fill="none"/>
    </svg>"#;
    let mut out = [0u8; 40 * 40 * 4];
    decode_svg(svg, &mut out).expect("path stroke failed");

    // Pixel along top stroke: (20, 5)
    let top_idx = (5 * 40 + 20) * 4;
    assert!(out[top_idx] > 200, "Red channel on top stroke");
    assert_eq!(out[top_idx + 3], 255);

    // Pixel far away: (10, 25) must be transparent
    let far_idx = (25 * 40 + 10) * 4;
    assert_eq!(&out[far_idx..far_idx + 4], &[0, 0, 0, 0]);
}

#[test]
fn test_porter_duff_alpha_compositing() {
    // Red background, semi-transparent white on top
    let svg = br##"<svg width="20" height="20">
        <rect x="0" y="0" width="20" height="20" fill="#ff0000"/>
        <rect x="0" y="0" width="20" height="20" fill="#ffffff80"/>
    </svg>"##;
    let mut out = [0u8; 20 * 20 * 4];
    decode_svg(svg, &mut out).expect("alpha compositing failed");

    let p = (10 * 20 + 10) * 4;
    // Expected blend: R blended with white at 128/255 alpha (~50%)
    // R: (255 * 128 + 255 * 255 * 127 / 255) / 255 = 255
    // G: (255 * 128 + 0) / 255 = 128
    // B: (255 * 128 + 0) / 255 = 128
    // A: 255
    assert_eq!(out[p], 255);
    assert!((out[p + 1] as i32 - 128).abs() <= 2, "G channel should be ~128");
    assert!((out[p + 2] as i32 - 128).abs() <= 2, "B channel should be ~128");
    assert_eq!(out[p + 3], 255);
}

#[test]
fn test_render_ellipse() {
    let svg = br#"<svg width="40" height="30">
        <ellipse cx="20" cy="15" rx="15" ry="8" fill="red"/>
    </svg>"#;
    let mut out = [0u8; 40 * 30 * 4];
    decode_svg(svg, &mut out).expect("ellipse decode failed");

    // Center (20, 15) must be red
    let center_idx = (15 * 40 + 20) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[255, 0, 0, 255]);

    // (20, 2) is distance 13 along y-axis > ry=8 -> transparent
    let out_y = (2 * 40 + 20) * 4;
    assert_eq!(&out[out_y..out_y + 4], &[0, 0, 0, 0]);

    // (2, 15) is distance 18 along x-axis > rx=15 -> transparent
    let out_x = (15 * 40 + 2) * 4;
    assert_eq!(&out[out_x..out_x + 4], &[0, 0, 0, 0]);
}

#[test]
fn test_render_line() {
    let svg = br#"<svg width="30" height="30">
        <line x1="5" y1="15" x2="25" y2="15" stroke="blue" stroke-width="2"/>
    </svg>"#;
    let mut out = [0u8; 30 * 30 * 4];
    decode_svg(svg, &mut out).expect("line decode failed");

    // Middle of line (15, 15) must have blue
    let mid_idx = (15 * 30 + 15) * 4;
    assert!(out[mid_idx + 2] > 200, "Blue channel on line");
    assert!(out[mid_idx + 3] > 200, "Alpha channel on line");

    // Away from line (15, 5) must be transparent
    let away_idx = (5 * 30 + 15) * 4;
    assert_eq!(&out[away_idx..away_idx + 4], &[0, 0, 0, 0]);
}

#[test]
fn test_render_polyline_and_polygon() {
    // Polyline stroke
    let polyline_svg = br#"<svg width="40" height="40">
        <polyline points="5,5 35,5 35,35" fill="none" stroke="green" stroke-width="2"/>
    </svg>"#;
    let mut out_pl = [0u8; 40 * 40 * 4];
    decode_svg(polyline_svg, &mut out_pl).expect("polyline decode failed");

    let top_idx = (5 * 40 + 20) * 4;
    assert!(out_pl[top_idx + 1] > 100, "Green channel on top segment");

    // For polyline, the closing diagonal (5,5 to 35,35) should NOT be stroked
    let mid_idx = (20 * 40 + 20) * 4;
    assert_eq!(&out_pl[mid_idx..mid_idx + 4], &[0, 0, 0, 0]);

    // Polygon fill and stroke (closed)
    let polygon_svg = br#"<svg width="40" height="40">
        <polygon points="5,5 35,5 35,35 5,35" fill="yellow" stroke="red" stroke-width="2"/>
    </svg>"#;
    let mut out_pg = [0u8; 40 * 40 * 4];
    decode_svg(polygon_svg, &mut out_pg).expect("polygon decode failed");

    let center_idx = (20 * 40 + 20) * 4;
    assert_eq!(&out_pg[center_idx..center_idx + 4], &[255, 255, 0, 255]);

    // Closing edge from (5,35) to (5,5) should have red stroke
    let close_edge = (20 * 40 + 5) * 4;
    assert!(out_pg[close_edge] > 200, "Red channel on closing edge");
}

#[test]
fn test_fill_rule_evenodd_vs_nonzero() {
    // Two concentric squares clockwise:
    // Outer: 0,0 to 30,30
    // Inner: 10,10 to 20,20
    let nonzero_svg = br#"<svg width="30" height="30">
        <path d="M 0 0 H 30 V 30 H 0 Z M 10 10 H 20 V 20 H 10 Z" fill="red" fill-rule="nonzero"/>
    </svg>"#;
    let mut out_nz = [0u8; 30 * 30 * 4];
    decode_svg(nonzero_svg, &mut out_nz).expect("nonzero decode failed");
    let center_idx = (15 * 30 + 15) * 4;
    // With nonzero, center is filled (winding = 2)
    assert_eq!(&out_nz[center_idx..center_idx + 4], &[255, 0, 0, 255]);

    let evenodd_svg = br#"<svg width="30" height="30">
        <path d="M 0 0 H 30 V 30 H 0 Z M 10 10 H 20 V 20 H 10 Z" fill="red" fill-rule="evenodd"/>
    </svg>"#;
    let mut out_eo = [0u8; 30 * 30 * 4];
    decode_svg(evenodd_svg, &mut out_eo).expect("evenodd decode failed");
    // With evenodd, center is hollow / transparent (crossings = 2, which is even)
    assert_eq!(&out_eo[center_idx..center_idx + 4], &[0, 0, 0, 0]);

    // Outer region (5, 5) is still filled with evenodd
    let outer_idx = (5 * 30 + 5) * 4;
    assert_eq!(&out_eo[outer_idx..outer_idx + 4], &[255, 0, 0, 255]);
}

#[test]
fn test_transforms() {
    // Translate
    let tr_svg = br#"<svg width="40" height="40">
        <rect x="0" y="0" width="10" height="10" transform="translate(15, 15)" fill="blue"/>
    </svg>"#;
    let mut out_tr = [0u8; 40 * 40 * 4];
    decode_svg(tr_svg, &mut out_tr).expect("translate decode failed");
    let in_idx = (20 * 40 + 20) * 4;
    assert_eq!(&out_tr[in_idx..in_idx + 4], &[0, 0, 255, 255]);
    let out_idx = (5 * 40 + 5) * 4;
    assert_eq!(&out_tr[out_idx..out_idx + 4], &[0, 0, 0, 0]);

    // Scale
    let sc_svg = br#"<svg width="40" height="40">
        <rect x="0" y="0" width="10" height="10" transform="scale(2)" fill="green"/>
    </svg>"#;
    let mut out_sc = [0u8; 40 * 40 * 4];
    decode_svg(sc_svg, &mut out_sc).expect("scale decode failed");
    let sc_in = (15 * 40 + 15) * 4;
    assert_eq!(&out_sc[sc_in..sc_in + 4], &[0, 128, 0, 255]);
    let sc_out = (25 * 40 + 25) * 4;
    assert_eq!(&out_sc[sc_out..sc_out + 4], &[0, 0, 0, 0]);

    // Matrix
    let mat_svg = br#"<svg width="40" height="40">
        <rect x="0" y="0" width="10" height="10" transform="matrix(2, 0, 0, 2, 5, 5)" fill="yellow"/>
    </svg>"#;
    let mut out_mat = [0u8; 40 * 40 * 4];
    decode_svg(mat_svg, &mut out_mat).expect("matrix decode failed");
    let mat_in = (10 * 40 + 10) * 4;
    assert_eq!(&out_mat[mat_in..mat_in + 4], &[255, 255, 0, 255]);

    // Group transform inheritance
    let g_svg = br#"<svg width="40" height="40">
        <g transform="translate(10, 10)">
            <rect x="0" y="0" width="10" height="10" transform="translate(5, 5)" fill="cyan"/>
        </g>
    </svg>"#;
    let mut out_g = [0u8; 40 * 40 * 4];
    decode_svg(g_svg, &mut out_g).expect("g transform decode failed");
    let g_in = (20 * 40 + 20) * 4;
    assert_eq!(&out_g[g_in..g_in + 4], &[0, 255, 255, 255]);
}

#[test]
fn test_security_and_reject_paths() {
    let mut out = [0u8; 20 * 20 * 4];

    // Script tag rejection
    let script_svg = br#"<svg width="20" height="20"><script>alert(1)</script></svg>"#;
    assert_eq!(decode_svg(script_svg, &mut out), Err(()));

    // External href rejection
    let ext_svg = br#"<svg width="20" height="20"><image href="http://evil.com/x.png"/></svg>"#;
    assert_eq!(decode_svg(ext_svg, &mut out), Err(()));

    // Malformed odd points polygon
    let odd_pts = br#"<svg width="20" height="20"><polygon points="5,5 10"/></svg>"#;
    assert_eq!(decode_svg(odd_pts, &mut out), Err(()));

    // An element that can carry content this reader cannot draw is refused
    // rather than skipped: skipping it would paint a partial picture and
    // call it the file.
    let foreign_svg = br#"<svg width="20" height="20"><foreignObject/><rect width="20" height="20" fill="red"/></svg>"#;
    assert_eq!(decode_svg(foreign_svg, &mut out), Err(()));
    // The refusal leaves no pixels behind.
    assert_eq!(&out[0..4], &[0, 0, 0, 0]);

    // An element that cannot change what is drawn is skipped, not refused.
    let inert_svg = br#"<svg width="20" height="20"><title>a title</title><desc>a description</desc><rect width="20" height="20" fill="red"/></svg>"#;
    assert_eq!(decode_svg(inert_svg, &mut out).map(|(w, h, _)| (w, h)), Ok((20, 20)));
    assert_eq!(&out[0..4], &[255, 0, 0, 255]);
}

#[test]
fn test_a_paint_server_is_refused_rather_than_faked() {
    let mut out = [0u8; 20 * 20 * 4];

    // A gradient is a paint server this reader does not implement. Drawing
    // a black fill where the file asked for a gradient would be inventing a
    // picture, so the document is refused.
    let gradient_svg = br##"<svg width="20" height="20"><defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/></linearGradient></defs><rect width="20" height="20" fill="url(#g)"/></svg>"##;
    assert_eq!(decode_svg(gradient_svg, &mut out), Err(()));

    // The same answer for a gradient that is named but never defined, and
    // for a filter that would have to be expanded.
    let undefined_svg = br#"<svg width="20" height="20"><rect width="20" height="20" fill="url(#missing)"/></svg>"#;
    assert_eq!(decode_svg(undefined_svg, &mut out), Err(()));

    let filter_svg = br#"<svg width="20" height="20"><rect width="20" height="20" filter="url(#f)"/></svg>"#;
    assert_eq!(decode_svg(filter_svg, &mut out), Err(()));
}

#[test]
fn test_nothing_outside_the_document_is_fetched() {
    let mut out = [0u8; 20 * 20 * 4];

    // An element reference is the way an SVG pulls another element into the
    // document. This reader expands none, so it refuses the document.
    let use_svg = br##"<svg width="20" height="20"><defs><rect id="r" width="20" height="20"/></defs><use href="#r"/></svg>"##;
    assert_eq!(decode_svg(use_svg, &mut out), Err(()));

    // A link that names another host is a reference outside the document.
    let link_svg = br#"<svg width="20" height="20"><a href="https://example.com/x"><rect width="20" height="20"/></a></svg>"#;
    assert_eq!(decode_svg(link_svg, &mut out), Err(()));

    // A stylesheet can import a remote sheet, and this reader applies no
    // stylesheets at all, so it refuses the document.
    let style_svg = br#"<svg width="20" height="20"><style>@import url("http://evil.com/x.css");</style><rect width="20" height="20"/></svg>"#;
    assert_eq!(decode_svg(style_svg, &mut out), Err(()));

    // A reference inside the same document is not a fetch, but the element
    // that asks for it is one this reader does not implement.
    let same_document_svg = br#"<svg width="20" height="20"><text x="0" y="10">hello</text></svg>"#;
    assert_eq!(decode_svg(same_document_svg, &mut out), Err(()));
}

#[test]
fn test_the_document_is_bounded() {
    let mut out = [0u8; 20 * 20 * 4];

    // Groups: the reader walks the document without recursion and refuses a
    // document with more groups than its style stack can carry, rather than
    // drawing the deeper ones with the wrong inherited style.
    let mut nested = Vec::from(&b"<svg width=\"20\" height=\"20\">"[..]);
    for _ in 0..(MAX_GROUP_DEPTH + 4) {
        nested.extend_from_slice(b"<g>");
    }
    nested.extend_from_slice(b"<rect width=\"20\" height=\"20\"/>");
    for _ in 0..(MAX_GROUP_DEPTH + 4) {
        nested.extend_from_slice(b"</g>");
    }
    nested.extend_from_slice(b"</svg>");
    assert_eq!(decode_svg(&nested, &mut out), Err(()));

    // Shapes: every shape costs a pass over the canvas it touches.
    let mut many = Vec::from(&b"<svg width=\"20\" height=\"20\">"[..]);
    for _ in 0..(MAX_DRAW_ELEMENTS + 1) {
        many.extend_from_slice(b"<rect width=\"2\" height=\"2\"/>");
    }
    many.extend_from_slice(b"</svg>");
    assert_eq!(decode_svg(&many, &mut out), Err(()));
}

#[test]
fn test_a_failed_decode_leaves_no_picture_behind() {
    let mut out = [0xFFu8; 20 * 20 * 4];

    // A document that is refused half way through must not leave the
    // shapes it had already drawn in the caller's buffer.
    let broken = br#"<svg width="20" height="20"><rect width="20" height="20" fill="red"/><path d="M 5 5 X 10 10"/></svg>"#;
    assert_eq!(decode_svg(broken, &mut out), Err(()));
    assert!(
        out.iter().all(|&byte| byte == 0),
        "a refused document must leave no pixels behind"
    );
}

#[test]
fn test_inline_svg_dom_layout_participation() {
    // HTML containing inline SVG inside text / elements
    let html = br#"
        <div>
            <p>Header text before SVG</p>
            <svg width="60" height="30" viewBox="0 0 120 60">
                <rect width="120" height="60" fill="red"/>
                <circle cx="60" cy="30" r="20" fill="blue"/>
            </svg>
            <p>Footer text after SVG</p>
        </div>
    "#;

    // Header dimensions decoded from inline SVG element
    let (w, h) = decode_svg_header(html).expect("inline SVG header must decode");
    assert_eq!(w, 60);
    assert_eq!(h, 30);

    // Intrinsic aspect-ratio sizing from viewBox when one dimension is omitted
    let html_w_only = br#"<svg width="60" viewBox="0 0 200 100"><rect width="200" height="100"/></svg>"#;
    let (w1, h1) = decode_svg_header(html_w_only).expect("width-only with viewBox");
    assert_eq!(w1, 60);
    assert_eq!(h1, 30); // 60 * 100 / 200 = 30

    let html_h_only = br#"<svg height="40" viewBox="0 0 200 100"><rect width="200" height="100"/></svg>"#;
    let (w2, h2) = decode_svg_header(html_h_only).expect("height-only with viewBox");
    assert_eq!(w2, 80); // 40 * 200 / 100 = 80
    assert_eq!(h2, 40);

    let html_vb_only = br#"<svg viewBox="0 0 160 90"><rect width="160" height="90"/></svg>"#;
    let (w3, h3) = decode_svg_header(html_vb_only).expect("viewBox-only");
    assert_eq!(w3, 160);
    assert_eq!(h3, 90);

    // Render buffer sized to the layout dimensions
    let mut out = vec![0u8; (w * h * 4) as usize];
    let (dec_w, dec_h, len) = decode_svg(html, &mut out).expect("inline SVG must render into layout buffer");
    assert_eq!(dec_w, 60);
    assert_eq!(dec_h, 30);
    assert_eq!(len, 60 * 30 * 4);

    // Center pixel of layout box (30, 15) must contain blue circle
    let center_idx = (15 * 60 + 30) * 4;
    assert_eq!(&out[center_idx..center_idx + 4], &[0, 0, 255, 255]);
}

#[test]
fn decodes_svg_fixtures_against_sidecars() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svg");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("svg"))
        .collect();
    entries.sort();

    let mut total_diffs = 0;
    let mut failures = Vec::new();
    for path in &entries {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let data = std::fs::read(path).unwrap();

        // Documented bounds check: overflow cases are marked structurally-only
        if name == "bounds_overflow" {
            assert!(decode_svg_header(&data).is_err(), "bounds_overflow exceeding MAX_WIDTH must fail header decode");
            continue;
        }

        let rgba_path = path.with_extension("rgba");
        assert!(rgba_path.exists(), "Missing sidecar for {:?}", path);
        let expected = std::fs::read(&rgba_path).unwrap();
        let width = u32::from_le_bytes(expected[0..4].try_into().unwrap());
        let height = u32::from_le_bytes(expected[4..8].try_into().unwrap());
        let exp_pixels = &expected[8..];

        // Test decode_svg_header
        assert_eq!(decode_svg_header(&data), Ok((width, height)), "{path:?}");

        // Test decode_svg
        let mut out = vec![0u8; (width * height * 4) as usize];
        let res = decode_svg(&data, &mut out);
        assert!(res.is_ok(), "decode_svg failed for {path:?}: {res:?}");
        let (dec_w, dec_h, dec_len) = res.unwrap();
        assert_eq!(dec_w, width);
        assert_eq!(dec_h, height);
        assert_eq!(dec_len, out.len());

        let mut diff_count = 0;
        for (&actual, &exp) in out.iter().zip(exp_pixels.iter()) {
            if actual != exp {
                diff_count += 1;
            }
        }
        eprintln!(
            "Fixture {:<20} {:>4}x{:<4} {:>5} / {:>5} differing bytes",
            name, width, height, diff_count, exp_pixels.len()
        );
        if diff_count > 0 {
            failures.push(name.to_string());
        }
        total_diffs += diff_count;
    }
    assert!(entries.len() >= 6, "Expected at least 6 fixtures, saw {}", entries.len());
    assert_eq!(failures, Vec::<String>::new(), "Pixel mismatch in fixtures: {failures:?}");
    assert_eq!(total_diffs, 0, "All fixture pixels must match exactly");
}
