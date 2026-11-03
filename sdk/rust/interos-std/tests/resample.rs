// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Integration tests for the native PCM audio resampler.

use interos_std::resample::{ResampleError, Resampler, TARGET_CHANNELS};

#[test]
fn invalid_configuration_is_refused() {
    assert_eq!(
        Resampler::new(0, 2).err(),
        Some(ResampleError::InvalidSampleRate)
    );
    assert_eq!(
        Resampler::new(500_000, 2).err(),
        Some(ResampleError::InvalidSampleRate)
    );
    assert_eq!(
        Resampler::new(44_100, 0).err(),
        Some(ResampleError::UnsupportedChannels(0))
    );
    assert_eq!(
        Resampler::new(44_100, 3).err(),
        Some(ResampleError::UnsupportedChannels(3))
    );
}

#[test]
fn exact_48khz_stereo_passthrough_exact_bit_identity() {
    let mut resampler = Resampler::new_stereo(48_000).expect("48 kHz stereo should construct");
    assert!(resampler.is_passthrough());

    // Generate arbitrary stereo test data
    let mut input = Vec::with_capacity(48_000 * 2);
    for i in 0..48_000 {
        let left = ((i * 37) % 65536) as i16;
        let right = (-(i as i32 * 19) % 32768) as i16;
        input.push(left);
        input.push(right);
    }

    let mut output = vec![0i16; 48_000 * 2];
    let mut consumed_frames = 0;
    let mut written_frames = 0;

    // Test in chunks to ensure chunking preserves exact bit identity
    let chunk_frames = 512;
    while consumed_frames < 48_000 {
        let in_start = consumed_frames * 2;
        let in_end = (in_start + chunk_frames * 2).min(input.len());
        let out_start = written_frames * 2;
        let out_end = (out_start + chunk_frames * 2).min(output.len());

        let (c, w) = resampler.process(&input[in_start..in_end], &mut output[out_start..out_end]);
        assert_eq!(c, w);
        consumed_frames += c;
        written_frames += w;
    }

    assert_eq!(consumed_frames, 48_000);
    assert_eq!(written_frames, 48_000);
    assert_eq!(
        output, input,
        "48 kHz stereo passthrough must have exact bit identity"
    );
}

#[test]
fn mono_48khz_duplicates_left_to_right() {
    let mut resampler = Resampler::new_mono(48_000).expect("48 kHz mono should construct");
    assert!(!resampler.is_passthrough());

    let input: Vec<i16> = (0..1000).map(|i| (i * 29 - 15000) as i16).collect();
    let mut output = vec![0i16; 1000 * 2];

    let (c, w) = resampler.process(&input, &mut output);
    assert_eq!(c, 1000);
    assert_eq!(w, 1000);

    for i in 0..1000 {
        let l = output[i * 2];
        let r = output[i * 2 + 1];
        assert_eq!(l, input[i], "Left sample matches input");
        assert_eq!(r, input[i], "Right sample matches input");
        assert_eq!(l, r, "Left equals Right");
    }
}

#[test]
fn mono_44_1khz_duplicates_left_to_right() {
    let mut resampler = Resampler::new_mono(44_100).expect("44.1 kHz mono should construct");

    let input: Vec<i16> = (0..4410)
        .map(|i| (10000.0 * ((i as f64 * 0.1).sin())) as i16)
        .collect();
    let mut output = vec![0i16; 4800 * 2];

    let mut in_pos = 0;
    let mut out_pos = 0;
    while in_pos < input.len() {
        let (c, w) = resampler.process(&input[in_pos..], &mut output[out_pos..]);
        if c == 0 && w == 0 {
            break;
        }
        in_pos += c;
        out_pos += w * 2;
    }

    let drained = resampler.drain(&mut output[out_pos..]);
    out_pos += drained * 2;

    let frames = out_pos / 2;
    assert!(frames >= 4799 && frames <= 4801, "produced {frames} frames");

    for i in 0..frames {
        let l = output[i * 2];
        let r = output[i * 2 + 1];
        assert_eq!(
            l, r,
            "Mono input must duplicate identically to L and R at frame {i}"
        );
    }
}

#[test]
fn dc_preservation_constant_input_stays_strictly_constant() {
    let dc_values = [0i16, 1i16, -1i16, 1000i16, -1000i16, 32767i16, -32768i16];

    for &dc in &dc_values {
        let mut resampler = Resampler::new_stereo(44_100).expect("resampler constructs");
        let input = vec![dc; 4410 * 2]; // 4410 frames = 100 ms
        let mut output = vec![0i16; 4800 * 2];

        let mut in_pos = 0;
        let mut out_pos = 0;
        while in_pos < input.len() {
            let in_end = (in_pos + 512 * 2).min(input.len());
            let out_end = (out_pos + 512 * 2).min(output.len());
            let (c, w) = resampler.process(
                &input[in_pos..in_end],
                &mut output[out_pos..out_end],
            );
            in_pos += c * 2;
            out_pos += w * 2;
            if c == 0 && w == 0 {
                break;
            }
        }
        let drained = resampler.drain(&mut output[out_pos..]);
        out_pos += drained * 2;

        let frames = out_pos / 2;
        for i in 0..frames {
            assert_eq!(
                output[i * 2],
                dc,
                "DC Left mismatch at frame {i} for value {dc}"
            );
            assert_eq!(
                output[i * 2 + 1],
                dc,
                "DC Right mismatch at frame {i} for value {dc}"
            );
        }
    }
}

#[test]
fn resample_44_1khz_to_48khz_stereo_length_and_continuity() {
    let mut resampler = Resampler::new_stereo(44_100).expect("44.1 kHz stereo should construct");

    // 44,100 input frames (1.000 second of audio at 44.1 kHz)
    // A 440 Hz stereo tone (Left = 440 Hz, Right = 880 Hz)
    let mut input = Vec::with_capacity(44_100 * 2);
    for i in 0..44_100 {
        let t = i as f64 / 44_100.0;
        let l = (15000.0 * (2.0 * core::f64::consts::PI * 440.0 * t).sin()) as i16;
        let r = (15000.0 * (2.0 * core::f64::consts::PI * 880.0 * t).sin()) as i16;
        input.push(l);
        input.push(r);
    }

    let mut output = vec![0i16; 48_050 * 2];
    let mut in_pos = 0;
    let mut out_pos = 0;

    // Process in arbitrary chunks (e.g. 576 frames like MP3 granule)
    let chunk_size = 576 * 2;
    while in_pos < input.len() {
        let in_end = (in_pos + chunk_size).min(input.len());
        let (c, w) = resampler.process(&input[in_pos..in_end], &mut output[out_pos..]);
        in_pos += c * 2;
        out_pos += w * 2;
        if c == 0 && w == 0 {
            break;
        }
    }

    let drained = resampler.drain(&mut output[out_pos..]);
    out_pos += drained * 2;

    let total_frames = out_pos / 2;
    assert_eq!(in_pos, input.len(), "all input frames consumed");
    // 44,100 frames at 44.1 kHz must produce exactly 48,000 frames at 48 kHz
    assert!(
        (total_frames as i64 - 48_000).abs() <= 1,
        "expected ~48,000 frames, got {total_frames}"
    );

    // Verify waveform continuity: consecutive sample delta must be smooth and bounded.
    // For 440 Hz at amplitude 15000: max delta between consecutive 48 kHz samples is:
    // 15000 * 2 * pi * 440 / 48000 ≈ 864.
    // Any discontinuity (e.g. duplicated or dropped frame) would show a large jump.
    for i in 1..total_frames {
        let delta_l = (output[i * 2] as i32 - output[(i - 1) * 2] as i32).abs();
        let delta_r = (output[i * 2 + 1] as i32 - output[(i - 1) * 2 + 1] as i32).abs();
        assert!(
            delta_l <= 1200,
            "Left channel discontinuity detected at frame {i}: delta={delta_l}"
        );
        assert!(
            delta_r <= 2400,
            "Right channel discontinuity detected at frame {i}: delta={delta_r}"
        );
    }
}

#[test]
fn resample_32khz_to_48khz_stereo_length() {
    let mut resampler = Resampler::new_stereo(32_000).expect("32 kHz stereo should construct");

    // 32,000 frames at 32 kHz is exactly 1.000 second of audio.
    // Converted to 48 kHz, it should produce exactly 48,000 frames (1.5x ratio).
    let mut input = Vec::with_capacity(32_000 * 2);
    for i in 0..32_000 {
        let t = i as f64 / 32_000.0;
        let l = (10000.0 * (2.0 * core::f64::consts::PI * 300.0 * t).sin()) as i16;
        let r = (10000.0 * (2.0 * core::f64::consts::PI * 600.0 * t).sin()) as i16;
        input.push(l);
        input.push(r);
    }

    let mut output = vec![0i16; 48_050 * 2];
    let mut in_pos = 0;
    let mut out_pos = 0;

    let chunk_size = 1024 * 2;
    while in_pos < input.len() {
        let in_end = (in_pos + chunk_size).min(input.len());
        let (c, w) = resampler.process(&input[in_pos..in_end], &mut output[out_pos..]);
        in_pos += c * 2;
        out_pos += w * 2;
        if c == 0 && w == 0 {
            break;
        }
    }

    let drained = resampler.drain(&mut output[out_pos..]);
    out_pos += drained * 2;

    let total_frames = out_pos / 2;
    assert_eq!(in_pos, input.len(), "all input frames consumed");
    assert!(
        (total_frames as i64 - 48_000).abs() <= 1,
        "32 kHz -> 48 kHz produced {total_frames} frames (expected 48,000)"
    );
}

#[test]
fn chunk_invariance_small_chunks_produce_identical_output_to_large_buffer() {
    let mut r_large = Resampler::new_stereo(44_100).unwrap();
    let mut r_small = Resampler::new_stereo(44_100).unwrap();

    let mut input = Vec::with_capacity(4410 * 2);
    for i in 0..4410 {
        let l = ((i * 123) % 20000 - 10000) as i16;
        let r = ((i * 321) % 20000 - 10000) as i16;
        input.push(l);
        input.push(r);
    }

    // Process whole buffer at once
    let mut out_large = vec![0i16; 4800 * 2];
    let (_c1, w1) = r_large.process(&input, &mut out_large);
    let d1 = r_large.drain(&mut out_large[w1 * 2..]);
    let total_large = w1 + d1;

    // Process in tiny chunks of 37 frames
    let mut out_small = vec![0i16; 4800 * 2];
    let mut in_pos = 0;
    let mut out_pos = 0;
    while in_pos < input.len() {
        let chunk_samples = 37 * 2;
        let in_end = (in_pos + chunk_samples).min(input.len());
        let (c, w) = r_small.process(&input[in_pos..in_end], &mut out_small[out_pos..]);
        in_pos += c * 2;
        out_pos += w * 2;
        if c == 0 && w == 0 {
            break;
        }
    }
    let d2 = r_small.drain(&mut out_small[out_pos..]);
    let total_small = (out_pos / 2) + d2;

    assert_eq!(total_large, total_small);
    assert_eq!(
        &out_large[..total_large * 2],
        &out_small[..total_small * 2],
        "Small-chunk processing must match large buffer bit-for-bit"
    );
}

#[test]
fn process_samples_wrapper_matches_process() {
    let mut r = Resampler::new_mono(44_100).unwrap();
    let input = [1000i16, 2000, 3000, 4000];
    let mut output = [0i16; 16];

    let (c_samples, w_samples) = r.process_samples(&input, &mut output);
    // 1 mono frame = 1 sample; 1 stereo frame = 2 samples
    assert!(c_samples > 0);
    assert!(w_samples > 0);
    assert_eq!(w_samples % TARGET_CHANNELS, 0);
}

#[test]
fn helper_getters_and_finish_and_drain_work() {
    let mut r = Resampler::new_stereo(44_100).unwrap();
    assert_eq!(r.src_rate(), 44_100);
    assert_eq!(r.src_channels(), 2);
    assert_eq!(r.target_rate(), 48_000);
    assert_eq!(r.target_channels(), 2);

    let input = [1000i16, -1000, 2000, -2000];
    let mut output = [0i16; 8];
    let _ = r.process(&input, &mut output);
    let drained = r.finish_and_drain(&mut output[2..]);
    assert!(r.is_finished());
    assert!(r.is_drained());
    let _ = drained;
}

#[test]
fn reset_allows_resampler_reuse() {
    let mut r = Resampler::new_stereo(44_100).unwrap();
    let input = [1000i16, -1000, 2000, -2000];
    let mut output = [0i16; 8];

    let _ = r.process(&input, &mut output);
    r.finish();
    assert!(r.is_finished());

    r.reset();
    assert!(!r.is_finished());
    let mut output2 = [0i16; 8];
    let (_, w) = r.process(&input, &mut output2);
    assert_eq!(w, 2);
    assert_eq!(output2[0], 1000);
    assert_eq!(output2[1], -1000);
}
