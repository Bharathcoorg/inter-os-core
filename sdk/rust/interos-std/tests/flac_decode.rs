//! End-to-end tests for the native FLAC decoder.
//!
//! Fixtures live in `tests/fixtures/flac/` and were produced with ffmpeg
//! (the tests themselves need no ffmpeg). Regenerate them with:
//!
//! ```text
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*660*t):s=44100:d=0.2:c=stereo" \
//!        -c:a flac -sample_fmt s16 stereo_44100_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|-0.5*sin(2*PI*440*t):s=44100:d=0.2:c=stereo" \
//!        -c:a flac -sample_fmt s16 stereo_ms_44100_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|0.25*sin(2*PI*440*t):s=44100:d=0.2:c=stereo" \
//!        -c:a flac -sample_fmt s16 stereo_rs_44100_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t):s=44100:d=0.2:c=mono" \
//!        -c:a flac -sample_fmt s16 mono_44100_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*660*t):s=48000:d=0.2:c=stereo" \
//!        -c:a flac -sample_fmt s16 stereo_48000_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*660*t):s=44100:d=0.2:c=stereo" \
//!        -c:a flac -sample_fmt s32 stereo_44100_s24.flac
//! ffmpeg -f lavfi -i "aevalsrc=sin(2*PI*440*t):s=44100:d=0.1:c=mono" \
//!        -c:a flac -sample_fmt s16 mono_fullscale_44100_s16.flac
//! ffmpeg -f lavfi -i "aevalsrc=0|0:s=44100:d=0.1:c=stereo" \
//!        -c:a flac -sample_fmt s16 silence_44100_s16.flac
//! ffmpeg -f lavfi -i "anoisesrc=color=white:amplitude=0.3:sample_rate=44100:duration=0.05" \
//!        -ac 2 -c:a flac -sample_fmt s16 noise_44100_s16.flac
//! ```
//!
//! The expected PCM hashes below are FNV-1a 64 over the interleaved
//! little-endian i16 samples that ffmpeg's own decoder produces (for the
//! 24-bit fixture the reference is `s32 >> 16`, matching this decoder's
//! documented truncation).

use interos_std::flac::{
    ChannelAssignment, FlacDecoder, FlacError, FrameInfo, StreamInfo, MAX_BLOCK_SIZE, MAX_CHANNELS,
};

#[derive(Debug)]
struct Decoded {
    pcm: Vec<i16>,
    frames: Vec<FrameInfo>,
    info: StreamInfo,
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/flac")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Runs the decoder over `file` in `chunk`-sized pushes, exactly like a
/// caller reading a capability-gated file sequentially.
fn decode_with(file: &[u8], chunk: usize) -> Result<Decoded, FlacError> {
    let mut decoder = FlacDecoder::new();
    let mut out = vec![0i16; MAX_BLOCK_SIZE * MAX_CHANNELS];
    let mut pcm = Vec::new();
    let mut frames = Vec::new();
    let mut at = 0usize;
    let mut finished = false;
    loop {
        if at < file.len() {
            let end = (at + chunk).min(file.len());
            let accepted = decoder.push(&file[at..end]);
            assert!(accepted > 0, "decoder stalled with a full buffer at byte {at}");
            at += accepted;
        } else if !finished {
            decoder.finish();
            finished = true;
        }
        loop {
            match decoder.next_frame(&mut out)? {
                Some(info) => {
                    assert!(info.samples <= out.len());
                    pcm.extend_from_slice(&out[..info.samples]);
                    frames.push(info);
                }
                None => break,
            }
        }
        if finished {
            break;
        }
    }
    let info = decoder.stream_info().expect("STREAMINFO was parsed");
    Ok(Decoded { pcm, frames, info })
}

fn decode(file: &[u8]) -> Result<Decoded, FlacError> {
    decode_with(file, 2048)
}

fn fnv1a(pcm: &[i16]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for sample in pcm {
        for byte in sample.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

fn frame_offset(file: &[u8]) -> usize {
    assert_eq!(&file[..4], b"fLaC");
    let mut at = 4;
    loop {
        assert!(at + 4 <= file.len());
        let last = file[at] & 0x80 != 0;
        let len = ((file[at + 1] as usize) << 16)
            | ((file[at + 2] as usize) << 8)
            | file[at + 3] as usize;
        at += 4 + len;
        if last {
            return at;
        }
    }
}

fn rms(samples: &[i16]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// The full-scale RMS of the half-scale sine the fixtures carry.
const HALF_SCALE_RMS: f64 = 11_585.25;

#[test]
fn metadata_before_the_first_frame_is_skipped() {
    // ffmpeg writes VORBIS_COMMENT and an 8 KiB PADDING block before the
    // frames; a decoder that stops at the first non-STREAMINFO block cannot
    // find the sync code at all.
    let file = fixture("stereo_44100_s16.flac");
    let offset = frame_offset(&file);
    assert!(offset > 8_000, "the fixture carries a multi-block metadata chain");
    let decoded = decode(&file).expect("metadata chain is skipped");
    assert_eq!(decoded.info.total_samples, 8_820);
    assert_eq!(decoded.pcm.len(), 8_820 * 2);
}

#[test]
fn an_application_block_spanning_pushes_is_skipped() {
    let file = fixture("stereo_44100_s16.flac");
    // Insert a 3000-byte APPLICATION block after STREAMINFO. At 2 KiB pushes
    // the block is not even fully buffered when decoding starts.
    let insert_at = 4 + 4 + 34;
    let body_len = 3_000usize;
    let mut patched = Vec::with_capacity(file.len() + 4 + body_len);
    patched.extend_from_slice(&file[..insert_at]);
    patched.push(0x02); // not last, type APPLICATION
    patched.push((body_len >> 16) as u8);
    patched.push((body_len >> 8) as u8);
    patched.push(body_len as u8);
    patched.extend_from_slice(b"tEsT");
    patched.extend(std::iter::repeat(0u8).take(body_len - 4));
    patched.extend_from_slice(&file[insert_at..]);

    let plain = decode(&file).unwrap();
    let patched = decode(&patched).unwrap();
    assert_eq!(patched.pcm, plain.pcm);
    assert_eq!(patched.info, plain.info);
}

#[test]
fn stereo_44100_reports_streaminfo_and_exact_frame_counts() {
    let decoded = decode(&fixture("stereo_44100_s16.flac")).unwrap();
    let info = decoded.info;
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.bits_per_sample, 16);
    assert_eq!(info.total_samples, 8_820);
    assert_eq!(info.max_block_size, 4_608);

    assert_eq!(decoded.pcm.len(), 8_820 * 2);
    // ceil(8820 / 4608) = 2 blocks: 4608 and 4212 samples.
    assert_eq!(decoded.frames.len(), 2);
    assert_eq!(decoded.frames[0].block_size, 4_608);
    assert_eq!(decoded.frames[1].block_size, 4_212);
    assert_eq!(decoded.frames[0].first_sample, 0);
    assert_eq!(decoded.frames[1].first_sample, 4_608);
    for frame in &decoded.frames {
        assert_eq!(frame.sample_rate, 44_100);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.bits_per_sample, 16);
        assert_eq!(frame.samples, frame.block_size as usize * 2);
        assert_eq!(frame.channel_assignment, ChannelAssignment::Independent);
    }
    assert_eq!(
        decoded.frames.iter().map(|f| f.block_size).sum::<u32>(),
        8_820
    );
    assert_eq!(fnv1a(&decoded.pcm), 0xa35b_0ab4_62d8_f1f5);
}

#[test]
fn stereo_44100_rms_is_within_one_percent_of_a_half_scale_sine() {
    let decoded = decode(&fixture("stereo_44100_s16.flac")).unwrap();
    let left: Vec<i16> = decoded.pcm.iter().step_by(2).copied().collect();
    let right: Vec<i16> = decoded.pcm.iter().skip(1).step_by(2).copied().collect();
    // The fixture is 0.5 full scale, so RMS ~= 0.5 * 32767 / sqrt(2). The
    // tolerance is 1% of that ideal; the source is a pure 440/660 Hz tone.
    for channel in [&left, &right] {
        let measured = rms(channel);
        assert!(
            (measured - HALF_SCALE_RMS).abs() <= HALF_SCALE_RMS * 0.01,
            "rms {measured} vs {HALF_SCALE_RMS}"
        );
    }
    assert_eq!(fnv1a(&decoded.pcm), 0xa35b_0ab4_62d8_f1f5);
}

#[test]
fn a_full_scale_sine_reaches_full_scale_rms() {
    let decoded = decode(&fixture("mono_fullscale_44100_s16.flac")).unwrap();
    assert_eq!(decoded.info.sample_rate, 44_100);
    assert_eq!(decoded.info.channels, 1);
    assert_eq!(decoded.pcm.len(), 4_410);
    // A full-scale sine has RMS 32768/sqrt(2) ~= 23170.5. The tolerance is
    // 1% of that ideal so this checks amplitude, not sample-by-sample luck.
    let ideal = 32_768.0 / 2f64.sqrt();
    let measured = rms(&decoded.pcm);
    assert!(
        (measured - ideal).abs() <= ideal * 0.01,
        "rms {measured} vs {ideal}"
    );
    assert_eq!(
        decoded.pcm.iter().map(|s| s.unsigned_abs()).max().unwrap(),
        32_768
    );
    assert_eq!(fnv1a(&decoded.pcm), 0xc412_84d1_63bf_c575);
}

#[test]
fn mono_44100_emits_one_sample_per_frame_position() {
    let decoded = decode(&fixture("mono_44100_s16.flac")).unwrap();
    assert_eq!(decoded.info.channels, 1);
    assert_eq!(decoded.info.sample_rate, 44_100);
    assert_eq!(decoded.pcm.len(), 8_820);
    assert_eq!(decoded.frames.len(), 2);
    for frame in &decoded.frames {
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.samples, frame.block_size as usize);
    }
    let measured = rms(&decoded.pcm);
    assert!(
        (measured - HALF_SCALE_RMS).abs() <= HALF_SCALE_RMS * 0.01,
        "rms {measured}"
    );
    assert_eq!(fnv1a(&decoded.pcm), 0x56ef_cf33_365a_67c5);
}

#[test]
fn stereo_48000_reports_48000_hz_and_the_expected_sample_count() {
    let decoded = decode(&fixture("stereo_48000_s16.flac")).unwrap();
    assert_eq!(decoded.info.sample_rate, 48_000);
    assert_eq!(decoded.info.total_samples, 9_600);
    assert_eq!(decoded.pcm.len(), 9_600 * 2);
    assert_eq!(decoded.frames.len(), 3);
    assert_eq!(
        decoded.frames.iter().map(|f| f.block_size).sum::<u32>(),
        9_600
    );
    assert!(decoded
        .frames
        .iter()
        .all(|f| f.sample_rate == 48_000 && f.channels == 2));
    assert_eq!(fnv1a(&decoded.pcm), 0xe3ec_c930_24c0_f6e5);
}

#[test]
fn mid_side_stereo_decodes_bit_exactly() {
    let decoded = decode(&fixture("stereo_ms_44100_s16.flac")).unwrap();
    assert_eq!(
        decoded.frames[0].channel_assignment,
        ChannelAssignment::MidSide
    );
    assert_eq!(fnv1a(&decoded.pcm), 0xf533_2939_0e81_fe75);
    let left = rms(&decoded.pcm.iter().step_by(2).copied().collect::<Vec<_>>());
    let right = rms(
        &decoded
            .pcm
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .collect::<Vec<_>>(),
    );
    assert!((left - HALF_SCALE_RMS).abs() < 1.0);
    assert!((right - HALF_SCALE_RMS).abs() < 1.0);
}

#[test]
fn right_side_stereo_decodes_bit_exactly() {
    let decoded = decode(&fixture("stereo_rs_44100_s16.flac")).unwrap();
    assert_eq!(
        decoded.frames[0].channel_assignment,
        ChannelAssignment::RightSide
    );
    assert_eq!(fnv1a(&decoded.pcm), 0x5954_456d_4fa2_bca5);
    // The fixture's right channel is half the amplitude of its left channel.
    let left = rms(&decoded.pcm.iter().step_by(2).copied().collect::<Vec<_>>());
    let right = rms(
        &decoded
            .pcm
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .collect::<Vec<_>>(),
    );
    assert!((left - HALF_SCALE_RMS).abs() < 1.0);
    assert!((right - HALF_SCALE_RMS / 2.0).abs() < 1.0);
}

#[test]
fn left_side_stereo_and_noise_residuals_decode_bit_exactly() {
    let decoded = decode(&fixture("noise_44100_s16.flac")).unwrap();
    assert_eq!(
        decoded.frames[0].channel_assignment,
        ChannelAssignment::LeftSide
    );
    assert_eq!(decoded.pcm.len(), 2_205 * 2);
    assert_eq!(fnv1a(&decoded.pcm), 0x9213_d912_5de0_1d15);
    assert_eq!(
        decoded
            .pcm
            .iter()
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap(),
        6_951
    );
}

#[test]
fn constant_subframes_decode_to_digital_silence() {
    let decoded = decode(&fixture("silence_44100_s16.flac")).unwrap();
    assert_eq!(decoded.pcm.len(), 4_410 * 2);
    assert!(decoded.pcm.iter().all(|&s| s == 0));
    assert_eq!(fnv1a(&decoded.pcm), 0x0397_2ecb_9416_bb45);
}

#[test]
fn twenty_four_bit_is_accepted_and_truncated_to_sixteen_bit() {
    let decoded = decode(&fixture("stereo_44100_s24.flac")).unwrap();
    assert_eq!(decoded.info.bits_per_sample, 24);
    assert_eq!(decoded.info.total_samples, 8_820);
    assert_eq!(decoded.frames[0].bits_per_sample, 24);
    assert_eq!(decoded.pcm.len(), 8_820 * 2);
    // Documented policy: 24-bit samples are shifted right by 8 bits
    // (arithmetic, i.e. truncation toward negative infinity), never scaled
    // with an implied gain.
    assert_eq!(fnv1a(&decoded.pcm), 0x9254_3018_2593_434d);

    let reference = decode(&fixture("stereo_44100_s16.flac")).unwrap();
    let max_diff = decoded
        .pcm
        .iter()
        .zip(&reference.pcm)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap();
    // ffmpeg's 24->16 conversion rounds; this decoder truncates.
    assert!(max_diff <= 1, "max difference {max_diff}");
}

#[test]
fn a_truncated_stream_is_refused_with_truncated() {
    let file = fixture("stereo_44100_s16.flac");
    let frames_at = frame_offset(&file);

    // Cut in the middle of the first frame.
    assert_eq!(
        decode(&file[..frames_at + 100]).unwrap_err(),
        FlacError::Truncated
    );
    // Cut exactly at a frame boundary: every frame that is present decodes,
    // but STREAMINFO promises more samples than the file holds.
    assert_eq!(
        decode(&file[..frames_at]).unwrap_err(),
        FlacError::Truncated
    );
    // Cut inside the metadata chain.
    assert_eq!(decode(&file[..20]).unwrap_err(), FlacError::Truncated);
}

#[test]
fn corrupt_frames_are_refused_with_corruption_errors() {
    let file = fixture("stereo_44100_s16.flac");
    let frames_at = frame_offset(&file);

    // The first frame's header CRC byte is the sixth byte of the frame.
    let mut bad_header = file.clone();
    bad_header[frames_at + 5] ^= 0xff;
    assert_eq!(
        decode(&bad_header).unwrap_err(),
        FlacError::CorruptHeaderCrc,
        "a damaged header CRC must be reported"
    );

    // The last byte of the file is the low byte of the final frame's CRC-16.
    let mut bad_frame = file.clone();
    let last = bad_frame.len() - 1;
    bad_frame[last] ^= 0xff;
    assert_eq!(
        decode(&bad_frame).unwrap_err(),
        FlacError::CorruptFrameCrc,
        "a damaged frame CRC must be reported"
    );

    // A damaged subframe payload is caught by the frame CRC (or by the
    // structural checks) and is never emitted as audio.
    let mut bad_payload = file.clone();
    bad_payload[frames_at + 200] ^= 0xff;
    assert!(matches!(
        decode(&bad_payload),
        Err(FlacError::CorruptFrameCrc | FlacError::CorruptFrame)
    ));
}

#[test]
fn ogg_encapsulated_flac_and_foreign_files_are_refused_distinctly() {
    let mut ogg = b"OggS".to_vec();
    ogg.extend(std::iter::repeat(0u8).take(64));
    let mut decoder = FlacDecoder::new();
    assert_eq!(decoder.push(&ogg), ogg.len());
    let mut out = [0i16; 8];
    assert_eq!(decoder.next_frame(&mut out), Err(FlacError::OggEncapsulated));

    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&[0; 64]);
    let mut decoder = FlacDecoder::new();
    decoder.push(&wav);
    assert_eq!(decoder.next_frame(&mut out), Err(FlacError::NotFlac));

    assert_eq!(
        decode(&[]).unwrap_err(),
        FlacError::Truncated,
        "an empty file is not a FLAC stream"
    );
}

#[test]
fn a_stream_with_32_bit_samples_is_refused() {
    // A minimal STREAMINFO with 32-bit samples and no frames. The decoder
    // must refuse before any sample is emitted: 32-bit (and any float
    // convention layered on it) is not down-shifted silently.
    let mut file = b"fLaC".to_vec();
    file.push(0x80); // last block, STREAMINFO
    file.extend_from_slice(&[0, 0, 34]);
    file.extend_from_slice(&4_608u16.to_be_bytes());
    file.extend_from_slice(&4_608u16.to_be_bytes());
    file.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let packed = ((44_100u64) << 44)
        | ((2u64 - 1) << 41)
        | ((32u64 - 1) << 36)
        | 0u64;
    file.extend_from_slice(&packed.to_be_bytes());
    file.extend_from_slice(&[0; 16]);

    let mut decoder = FlacDecoder::new();
    decoder.push(&file);
    let mut out = [0i16; 8];
    assert_eq!(
        decoder.next_frame(&mut out),
        Err(FlacError::UnsupportedSampleSize(32))
    );
    assert_eq!(decoder.stream_info(), None);
}

#[test]
fn an_output_buffer_too_small_is_refused_without_losing_the_frame() {
    let file = fixture("stereo_44100_s16.flac");
    let mut decoder = FlacDecoder::new();
    assert_eq!(decoder.push(&file), file.len());

    let mut small = [0i16; 8];
    assert_eq!(
        decoder.next_frame(&mut small),
        Err(FlacError::OutputTooSmall { needed: 4_608 * 2 })
    );

    let mut out = vec![0i16; MAX_BLOCK_SIZE * MAX_CHANNELS];
    let frame = decoder.next_frame(&mut out).unwrap().unwrap();
    assert_eq!(frame.block_size, 4_608);
    assert_eq!(frame.samples, 4_608 * 2);
    assert_eq!(frame.first_sample, 0);
}

#[test]
fn decoding_is_independent_of_push_size() {
    let file = fixture("stereo_ms_44100_s16.flac");
    let reference = decode_with(&file, 2_048).unwrap();
    for chunk in [1usize, 7, 61, 512, 4_096, file.len()] {
        let decoded = decode_with(&file, chunk).unwrap();
        assert_eq!(decoded.pcm, reference.pcm, "chunk size {chunk}");
        assert_eq!(decoded.frames.len(), reference.frames.len());
    }
}
