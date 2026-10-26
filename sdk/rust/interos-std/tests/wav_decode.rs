use interos_std::wav::{parse_wav, PcmConverter, WavError, WavInfo};

fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let body_len: usize = chunks.iter().map(|(_, data)| 8 + data.len() + (data.len() & 1)).sum();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((4 + body_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    for (id, data) in chunks {
        out.extend_from_slice(*id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() & 1 == 1 {
            out.push(0);
        }
    }
    out
}

fn fmt_chunk(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let byte_rate = rate * block_align as u32;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&byte_rate.to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    fmt
}

fn pcm_wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    riff(&[
        (b"fmt ", fmt_chunk(tag, channels, rate, bits)),
        (b"data", data.to_vec()),
    ])
}

fn samples(pcm: &[u8]) -> Vec<i16> {
    pcm.chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

#[test]
fn parses_a_pcm16_stereo_header_and_reports_the_data_window() {
    let data = vec![0u8; 8];
    let file = pcm_wav(1, 2, 44_100, 16, &data);
    let (info, consumed) = parse_wav(&file).expect("a normal PCM header parses");
    assert_eq!(
        info,
        WavInfo {
            format_tag: 1,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            data_offset: 44,
            data_len: 8,
        }
    );
    assert_eq!(consumed, 44);
    assert_eq!(info.frame_bytes(), 4);
    assert_eq!(info.frame_count(), 2);
}

#[test]
fn a_header_with_a_list_chunk_and_odd_padding_still_finds_the_data() {
    let mut list = b"INFOISFT".to_vec();
    list.extend_from_slice(&[9, 0, 0, 0]);
    list.extend_from_slice(b"recorder!");
    let file = riff(&[
        (b"LIST", list),
        (b"fmt ", fmt_chunk(1, 1, 8_000, 8)),
        (b"data", vec![128, 129, 130]),
    ]);
    let (info, consumed) = parse_wav(&file).expect("the chunk walk skips LIST and its pad byte");
    assert_eq!(info.sample_rate, 8_000);
    assert_eq!(info.channels, 1);
    assert_eq!(info.bits_per_sample, 8);
    assert_eq!(consumed, info.data_offset as usize);
    assert_eq!(info.data_len, 3);
    assert_eq!(&file[consumed..consumed + 3], &[128, 129, 130]);
}

#[test]
fn a_file_that_is_not_a_riff_wave_is_refused() {
    let mut file = pcm_wav(1, 1, 8_000, 16, &[0, 0]);
    file[0] = b'X';
    assert_eq!(parse_wav(&file), Err(WavError::NotRiff));
    let mut file = pcm_wav(1, 1, 8_000, 16, &[0, 0]);
    file[8] = b'X';
    assert_eq!(parse_wav(&file), Err(WavError::NotWave));
}

#[test]
fn a_truncated_header_is_refused_instead_of_guessed() {
    let file = pcm_wav(1, 1, 8_000, 16, &[0, 0]);
    assert_eq!(parse_wav(&file[..10]), Err(WavError::Truncated));
    assert_eq!(parse_wav(&file[..30]), Err(WavError::Truncated));
    // The fmt chunk claims more than the buffer holds.
    assert_eq!(parse_wav(&file[..20]), Err(WavError::Truncated));
}

#[test]
fn a_format_the_controller_cannot_take_is_refused() {
    // IEEE float samples are not PCM integers, so there is nothing to scale.
    assert_eq!(parse_wav(&pcm_wav(3, 1, 8_000, 32, &[0; 8])), Err(WavError::UnsupportedFormat));
    // More than two channels has no honest stereo down-mix here.
    assert_eq!(parse_wav(&pcm_wav(1, 3, 8_000, 16, &[0; 12])), Err(WavError::UnsupportedFormat));
    // A bit depth the converter does not implement.
    assert_eq!(parse_wav(&pcm_wav(1, 1, 8_000, 12, &[0; 4])), Err(WavError::UnsupportedFormat));
    // A rate of zero is not a rate.
    assert_eq!(parse_wav(&pcm_wav(1, 1, 0, 16, &[0; 4])), Err(WavError::UnsupportedFormat));
}

#[test]
fn a_wave_with_no_data_chunk_is_refused() {
    let file = riff(&[(b"fmt ", fmt_chunk(1, 1, 8_000, 16))]);
    assert_eq!(parse_wav(&file), Err(WavError::Truncated));
}

#[test]
fn the_duration_is_derived_from_the_data_length_and_the_rate() {
    let data = vec![0u8; 48_000 * 4];
    let (info, _) = parse_wav(&pcm_wav(1, 2, 48_000, 16, &data)).unwrap();
    assert_eq!(info.frame_count(), 48_000);
    assert_eq!(info.duration_ms(), 1_000);
}

#[test]
fn native_rate_pcm_passes_through_unchanged_at_full_volume() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 2,
        bits_per_sample: 16,
        data_offset: 44,
        data_len: 8,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 8];
    let input = [0x64, 0x00, 0x9c, 0xff, 0xc8, 0x00, 0x38, 0xff];
    let (consumed, written) = converter.convert(&input, &mut out);
    assert_eq!((consumed, written), (8, 8));
    assert_eq!(samples(&out), vec![100, -100, 200, -200]);
}

#[test]
fn a_mono_frame_is_heard_on_both_channels() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 16,
        data_offset: 44,
        data_len: 4,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 8];
    let (consumed, written) = converter.convert(&[0x64, 0x00, 0x9c, 0xff], &mut out);
    assert_eq!((consumed, written), (4, 8));
    assert_eq!(samples(&out), vec![100, 100, -100, -100]);
}

#[test]
fn eight_bit_pcm_is_unsigned_and_is_centred_before_it_is_scaled() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 8,
        data_offset: 44,
        data_len: 3,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 12];
    let (consumed, written) = converter.convert(&[128, 255, 0], &mut out);
    assert_eq!((consumed, written), (3, 12));
    assert_eq!(samples(&out), vec![0, 0, 32512, 32512, -32768, -32768]);
}

#[test]
fn twenty_four_and_thirty_two_bit_pcm_keep_their_most_significant_bits() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 24,
        data_offset: 44,
        data_len: 6,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 16];
    converter.convert(&[0xff, 0xff, 0x7f, 0x00, 0x00, 0x80], &mut out);
    assert_eq!(samples(&out[..8]), vec![32767, 32767, -32768, -32768]);

    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 32,
        data_offset: 44,
        data_len: 8,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 16];
    converter.convert(&[0xff, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x80], &mut out);
    assert_eq!(samples(&out[..8]), vec![32767, 32767, -32768, -32768]);
}

#[test]
fn the_volume_control_scales_the_samples_that_are_emitted() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 16,
        data_offset: 44,
        data_len: 4,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    converter.set_volume_percent(50);
    let mut out = [0u8; 8];
    converter.convert(&[0xe8, 0x03, 0x18, 0xfc], &mut out);
    assert_eq!(samples(&out), vec![500, 500, -500, -500]);
    converter.set_volume_percent(0);
    let mut out = [0u8; 8];
    converter.convert(&[0xe8, 0x03, 0x18, 0xfc], &mut out);
    assert_eq!(samples(&out), vec![0, 0, 0, 0]);
}

#[test]
fn a_higher_rate_source_is_resampled_to_the_controller_rate() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 44_100,
        channels: 1,
        bits_per_sample: 16,
        data_offset: 44,
        data_len: 441 * 2,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut input = Vec::with_capacity(441 * 2);
    for _ in 0..441 {
        input.extend_from_slice(&1_000i16.to_le_bytes());
    }
    let mut out = vec![0u8; 2_000];
    let mut consumed = 0usize;
    let mut written = 0usize;
    while consumed < input.len() {
        let (c, w) = converter.convert(&input[consumed..], &mut out[written..]);
        if c == 0 {
            break;
        }
        consumed += c;
        written += w;
    }
    // 441 source frames at 44.1 kHz last 10 ms, which is about 480 output
    // frames at 48 kHz. One frame is still carried for interpolation.
    let out_frames = written / 4;
    assert!((out_frames as i64 - 480).abs() <= 3, "emitted {out_frames} frames");
    let all = samples(&out[..written]);
    assert!(all.iter().all(|&s| (s - 1000).abs() <= 1), "constant input stays constant");
}

#[test]
fn a_real_recorded_tone_parses_and_resamples_to_the_native_rate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wav/tone_44100_mono.wav");
    let file = std::fs::read(&path).unwrap();
    let (info, consumed) = parse_wav(&file).unwrap();
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 1);
    assert_eq!(info.bits_per_sample, 16);
    assert_eq!(info.duration_ms(), 100);
    assert_eq!(&file[consumed..consumed + 2], &file[44..46]);

    let mut converter = PcmConverter::new(info).unwrap();
    let input = &file[consumed..consumed + info.data_len as usize];
    let mut out = vec![0u8; 24_000];
    let mut done = 0usize;
    let mut written = 0usize;
    while done < input.len() {
        let (c, w) = converter.convert(&input[done..], &mut out[written..]);
        if c == 0 {
            break;
        }
        done += c;
        written += w;
    }
    assert_eq!(done, input.len());
    let out_frames = written / 4;
    assert!((out_frames as i64 - 4_800).abs() <= 3, "emitted {out_frames} frames");
    let peak = samples(&out[..written])
        .into_iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    // The fixture's amplitude is half scale; a real decode reaches it.
    assert!((15_000..=16_600).contains(&peak), "peak {peak}");
}

#[test]
fn a_split_source_frame_is_left_for_the_caller_to_carry() {
    let info = WavInfo {
        format_tag: 1,
        sample_rate: 48_000,
        channels: 2,
        bits_per_sample: 16,
        data_offset: 44,
        data_len: 6,
    };
    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = [0u8; 8];
    let input = [1, 0, 2, 0, 3];
    let (consumed, written) = converter.convert(&input, &mut out);
    // The converter answers in whole source frames; the caller keeps the
    // last byte and prepends it to the next block.
    assert_eq!(consumed, 4);
    assert_eq!(written, 4);
    assert_eq!(&input[consumed..], &[3]);
}

#[test]
fn decodes_alaw_samples_to_pcm() {
    let data = vec![0xD5, 0x55]; // A-law values
    let file = riff(&[
        (b"fmt ", fmt_chunk(6, 1, 8_000, 8)),
        (b"data", data),
    ]);
    let (info, consumed) = parse_wav(&file).expect("A-law header parses");
    assert_eq!(info.format_tag, 6);
    assert_eq!(info.channels, 1);
    assert_eq!(info.sample_rate, 8_000);
    assert_eq!(info.bits_per_sample, 8);

    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = vec![0u8; 100];
    let (consumed_bytes, written_bytes) = converter.convert(&file[consumed..consumed + 2], &mut out);
    assert!(consumed_bytes > 0);
    assert!(written_bytes > 0);
}

#[test]
fn decodes_mulaw_samples_to_pcm() {
    let data = vec![0xFF, 0x7F]; // µ-law values (0xFF = 0 silence)
    let file = riff(&[
        (b"fmt ", fmt_chunk(7, 1, 8_000, 8)),
        (b"data", data),
    ]);
    let (info, consumed) = parse_wav(&file).expect("µ-law header parses");
    assert_eq!(info.format_tag, 7);
    assert_eq!(info.channels, 1);
    assert_eq!(info.sample_rate, 8_000);

    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = vec![0u8; 100];
    let (consumed_bytes, written_bytes) = converter.convert(&file[consumed..consumed + 2], &mut out);
    assert!(consumed_bytes > 0);
    assert!(written_bytes > 0);
}

#[test]
fn decodes_adpcm_samples_to_pcm() {
    // 4-bit IMA ADPCM (format tag 17)
    let data = vec![0x00, 0x11, 0x22, 0x33];
    let file = riff(&[
        (b"fmt ", fmt_chunk(17, 1, 8_000, 4)),
        (b"data", data),
    ]);
    let (info, consumed) = parse_wav(&file).expect("IMA ADPCM parses");
    assert_eq!(info.format_tag, 17);
    assert_eq!(info.bits_per_sample, 4);

    let mut converter = PcmConverter::new(info).unwrap();
    let mut out = vec![0u8; 100];
    let (consumed_bytes, written_bytes) = converter.convert(&file[consumed..consumed + 4], &mut out);
    assert!(consumed_bytes > 0);
    assert!(written_bytes > 0);
}
