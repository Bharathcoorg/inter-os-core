// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

use interos_std::image::{gif_animation_info, GifAnimation};

#[test]
fn decodes_composited_frames_from_libgif_encoder_output() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gif");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("gif") {
            continue;
        }
        seen += 1;
        let data = std::fs::read(&path).unwrap();
        let expected = std::fs::read(path.with_extension("rgba")).unwrap();
        let width = u32::from_le_bytes(expected[0..4].try_into().unwrap()) as usize;
        let height = u32::from_le_bytes(expected[4..8].try_into().unwrap()) as usize;
        let pixels = width * height * 4;

        let info = gif_animation_info(&data).unwrap();
        assert_eq!((info.width as usize, info.height as usize), (width, height), "{path:?}");
        assert_eq!(info.frame_count as usize, expected[8..].len() / pixels, "{path:?}");
        if path.file_name().unwrap() == "transparent.gif" {
            assert_eq!(info.loop_count, 2, "{path:?}");
        } else {
            assert_eq!(info.loop_count, 0, "{path:?}");
        }

        let mut animation = GifAnimation::new(&data).unwrap();
        let mut canvas = vec![0u8; pixels];
        let mut restore = vec![0u8; pixels];
        for index in 0..info.frame_count as usize {
            let frame = animation
                .decode_next(&mut canvas, &mut restore)
                .unwrap_or_else(|_| panic!("{path:?} frame {index}"));
            assert!(frame.width > 0 && frame.height > 0, "{path:?}");
            let start = 8 + index * pixels;
            assert_eq!(
                &canvas[..],
                &expected[start..start + pixels],
                "{path:?} frame {index}"
            );
        }
        assert!(animation.decode_next(&mut canvas, &mut restore).is_err(), "{path:?}");
        assert!(animation.is_finished(), "{path:?}");
    }
    assert_eq!(seen, 2);
}

#[test]
fn loops_with_screen_sized_buffers_like_the_gallery_viewer() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gif/basic.gif");
    let data = std::fs::read(&path).unwrap();
    let info = gif_animation_info(&data).unwrap();

    // The viewer hands the decoder screen-sized scratch buffers rather than
    // one frame rectangle, so the same stream must loop through them.
    let mut canvas = vec![0u8; 720 * 1440 * 4];
    let mut restore = vec![0u8; 720 * 1440 * 4];
    let mut animation = GifAnimation::new(&data).unwrap();
    let required = (info.width * info.height * 4) as usize;

    for _pass in 0..3 {
        for index in 0..info.frame_count {
            let frame = animation
                .decode_next(&mut canvas[..required], &mut restore[..required])
                .unwrap_or_else(|_| panic!("frame {index} in pass {_pass}"));
            assert!(frame.delay_cs > 0);
        }
        let err = animation
            .decode_next(&mut canvas[..required], &mut restore[..required]);
        assert!(err.is_err() && animation.is_finished());
        animation.rewind();
        canvas[..required].fill(0);
    }
}
