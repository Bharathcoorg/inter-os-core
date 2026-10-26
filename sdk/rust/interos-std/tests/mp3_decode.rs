//! End-to-end checks for the MPEG-1 Layer III decoder.
//!
//! The fixtures are real libmp3lame streams generated once with FFmpeg and
//! committed here, so the test run needs no encoder. Each fixture is a
//! 440 Hz sine of known amplitude, which makes both the length and the
//! level of the decoded PCM checkable without a golden file.

use interos_std::mp3::{FrameInfo, Mp3Decoder, Mp3Error, MAX_FRAME_SAMPLES};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mp3")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

#[derive(Debug)]
struct Decoded {
    frames: usize,
    info: Vec<FrameInfo>,
    pcm: Vec<i16>,
}

impl Decoded {
    fn channels(&self) -> usize {
        self.info.first().map(|i| i.channels as usize).unwrap_or(0)
    }

    fn sample_rate(&self) -> u32 {
        self.info.first().map(|i| i.sample_rate).unwrap_or(0)
    }

    fn samples_per_channel(&self) -> usize {
        self.pcm.len() / self.channels().max(1)
    }

    /// Root mean square of one channel over the middle `from..to` fraction of
    /// the stream, in units of a 16-bit full scale of 1.0.
    fn rms(&self, channel: usize, from: f32, to: f32) -> f32 {
        let ch = self.channels();
        let frames = self.samples_per_channel();
        let start = (frames as f32 * from) as usize * ch + channel;
        let end = ((frames as f32 * to) as usize * ch + channel).min(self.pcm.len());
        let mut sum = 0.0f64;
        let mut n = 0usize;
        let mut at = start;
        while at < end {
            let v = self.pcm[at] as f64 / 32768.0;
            sum += v * v;
            n += 1;
            at += ch;
        }
        if n == 0 {
            return 0.0;
        }
        (sum / n as f64).sqrt() as f32
    }

    /// Per-frame RMS of one channel, skipping the encoder delay and padding.
    fn frame_rms(&self, channel: usize, frame_limit: usize) -> Vec<f32> {
        let ch = self.channels();
        let frames = self.samples_per_channel() / MAX_FRAME_SAMPLES;
        let mut levels = Vec::new();
        for frame in 2..frames.saturating_sub(2).min(frame_limit) {
            let mut sum = 0.0f64;
            for i in 0..MAX_FRAME_SAMPLES {
                let v = self.pcm[(frame * MAX_FRAME_SAMPLES + i) * ch + channel] as f64 / 32768.0;
                sum += v * v;
            }
            levels.push((sum / MAX_FRAME_SAMPLES as f64).sqrt() as f32);
        }
        levels
    }
}

/// Feeds the file through the public API in the 2 KiB chunks the capability
/// gated filesystem hands the player, draining after every push.
fn decode(bytes: &[u8]) -> Result<Decoded, Mp3Error> {
    decode_with_chunk(bytes, 2048)
}

fn decode_with_chunk(bytes: &[u8], chunk: usize) -> Result<Decoded, Mp3Error> {
    let mut decoder = Mp3Decoder::new();
    let mut pcm = vec![0i16; MAX_FRAME_SAMPLES * 2];
    let mut decoded = Decoded {
        frames: 0,
        info: Vec::new(),
        pcm: Vec::new(),
    };
    let mut drain = |decoder: &mut Mp3Decoder, decoded: &mut Decoded| -> Result<(), Mp3Error> {
        loop {
            match decoder.next_frame(&mut pcm)? {
                Some(info) => {
                    let samples = info.samples_per_channel * info.channels as usize;
                    decoded.pcm.extend_from_slice(&pcm[..samples]);
                    decoded.info.push(info);
                    decoded.frames += 1;
                }
                None => return Ok(()),
            }
        }
    };
    for part in bytes.chunks(chunk) {
        decoder.push(part)?;
        drain(&mut decoder, &mut decoded)?;
    }
    decoder.finish();
    drain(&mut decoder, &mut decoded)?;
    Ok(decoded)
}

// The fixtures contain a 0.5-amplitude 440 Hz sine, so a perfect decode has an
// RMS of 0.5/sqrt(2) = 0.3536 of full scale. MP3 is lossy: the encoder's
// low-pass, its psychoacoustic quantisation and the decoder's f32 rounding all
// move the level a little, and the sentinel Xing frame adds one silent frame to
// a 0.25 s file. 25 % around the ideal level accepts those effects while still
// failing a decoder that produces silence, half level, or clipping noise.
const TONE_RMS: f32 = 0.3536;
const TONE_RMS_TOLERANCE: f32 = 0.25;

fn assert_tone_level(rms: f32, what: &str) {
    let low = TONE_RMS * (1.0 - TONE_RMS_TOLERANCE);
    let high = TONE_RMS * (1.0 + TONE_RMS_TOLERANCE);
    assert!(
        (low..=high).contains(&rms),
        "{what}: RMS {rms:.4} outside {low:.4}..={high:.4} (ideal {TONE_RMS})"
    );
}

#[test]
fn decodes_44_1_khz_stereo_with_the_expected_length_and_level() {
    let data = fixture("tone_44100_stereo_128k.mp3");
    let decoded = decode(&data).expect("a normal 44.1 kHz stereo stream decodes");

    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.sample_rate(), 44_100);
    for info in &decoded.info {
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.samples_per_channel, MAX_FRAME_SAMPLES);
    }

    // 0.25 s at 44.1 kHz is 11025 samples; with the LAME delay/padding frames
    // the stream holds at least 10 whole 1152-sample frames.
    assert!(
        decoded.frames >= 10,
        "only {} frames decoded from a 0.25 s stream",
        decoded.frames
    );
    assert_eq!(decoded.pcm.len(), decoded.frames * MAX_FRAME_SAMPLES * 2);
    assert_eq!(decoded.samples_per_channel(), decoded.frames * MAX_FRAME_SAMPLES);

    assert_tone_level(decoded.rms(0, 0.25, 0.75), "left channel");
    assert_tone_level(decoded.rms(1, 0.25, 0.75), "right channel");
    // A stereo sine must not collapse to one channel.
    let spread = (decoded.rms(0, 0.25, 0.75) - decoded.rms(1, 0.25, 0.75)).abs();
    assert!(spread < 0.02, "channels differ by {spread} RMS");
}

#[test]
fn decodes_44_1_khz_mono_at_the_same_level() {
    let data = fixture("tone_44100_mono_64k.mp3");
    let decoded = decode(&data).expect("a mono stream decodes");

    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.sample_rate(), 44_100);
    assert!(decoded.frames >= 10);
    assert_eq!(decoded.pcm.len(), decoded.frames * MAX_FRAME_SAMPLES);
    assert_tone_level(decoded.rms(0, 0.25, 0.75), "mono channel");
}

#[test]
fn decodes_48_khz_stereo_and_reports_the_stream_rate() {
    let data = fixture("tone_48000_stereo_128k.mp3");
    let decoded = decode(&data).expect("a 48 kHz stream decodes");

    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.sample_rate(), 48_000);
    assert!(decoded.frames >= 10, "{} frames", decoded.frames);
    assert_eq!(decoded.pcm.len(), decoded.frames * MAX_FRAME_SAMPLES * 2);
    assert_tone_level(decoded.rms(0, 0.25, 0.75), "48 kHz left channel");
    assert_tone_level(decoded.rms(1, 0.25, 0.75), "48 kHz right channel");
}

#[test]
fn skips_an_id3v2_tag_at_the_start_of_the_file() {
    let data = fixture("id3_44100_stereo_128k.mp3");
    assert_eq!(&data[..3], b"ID3", "fixture must carry an ID3v2 tag");
    let decoded = decode(&data).expect("an ID3-prefixed stream decodes");

    assert_eq!(decoded.sample_rate(), 44_100);
    assert_eq!(decoded.channels(), 2);
    assert!(decoded.frames >= 10);
    assert_tone_level(decoded.rms(0, 0.25, 0.75), "ID3-prefixed left channel");
}

#[test]
fn refuses_a_truncated_stream_instead_of_emitting_the_partial_frame() {
    let data = fixture("tone_44100_stereo_128k.mp3");
    // Cut inside the final frame: the header is present but the payload is not.
    let cut = &data[..data.len() - 700];
    let result = decode(cut);
    assert_eq!(
        result.err(),
        Some(Mp3Error::Truncated),
        "a stream that ends mid-frame must be refused as truncated"
    );

    // The prefix before the cut still has to decode, i.e. the error must come
    // from the missing tail and not from the truncation point being unparseable.
    let mut decoder = Mp3Decoder::new();
    let mut pcm = vec![0i16; MAX_FRAME_SAMPLES * 2];
    let mut frames = 0;
    for chunk in cut.chunks(2048) {
        decoder.push(chunk).unwrap();
        while let Some(_) = decoder.next_frame(&mut pcm).expect("buffered frames decode") {
            frames += 1;
        }
    }
    assert!(frames >= 8, "only {frames} frames before the cut");
    // Without finish(), the truncated trailing frame returns Ok(None) while waiting for data.
    assert_eq!(decoder.next_frame(&mut pcm).unwrap(), None);
    // Once finished, the partial trailing frame is refused deterministically as Truncated.
    decoder.finish();
    assert_eq!(
        decoder.next_frame(&mut pcm).err(),
        Some(Mp3Error::Truncated)
    );
}

#[test]
fn refuses_free_format_layer_one_two_and_corrupt_headers_with_distinct_errors() {
    // MPEG-1 Layer III, 128 kbps, 44.1 kHz, no CRC: the shape of a real frame.
    let valid = [0xFFu8, 0xFB, 0x90, 0x64];
    let free_format = [0xFFu8, 0xFB, 0x00, 0x64];
    let layer_two = [0xFFu8, 0xFD, 0x90, 0x64];
    let layer_one = [0xFFu8, 0xFF, 0x90, 0x64];
    // Version 00 is MPEG-2.5; unsupported version.
    let mpeg2_5 = [0xFFu8, 0xE3, 0x90, 0x64];
    // Version 10 is MPEG-2; supported version.
    let mpeg2 = [0xFFu8, 0xF3, 0x90, 0x64];

    let decode_header = |header: [u8; 4]| -> Result<Decoded, Mp3Error> {
        let mut decoder = Mp3Decoder::new();
        let mut pcm = vec![0i16; MAX_FRAME_SAMPLES * 2];
        decoder.push(&header)?;
        decoder.finish();
        let _ = decoder.next_frame(&mut pcm)?;
        Ok(Decoded {
            frames: 0,
            info: Vec::new(),
            pcm: Vec::new(),
        })
    };

    assert_eq!(decode_header(free_format).err(), Some(Mp3Error::FreeFormat));
    assert_eq!(decode_header(layer_two).err(), Some(Mp3Error::UnsupportedLayer));
    assert_eq!(decode_header(layer_one).err(), Some(Mp3Error::UnsupportedLayer));
    assert_eq!(
        decode_header(mpeg2_5).err(),
        Some(Mp3Error::UnsupportedVersion)
    );
    assert_eq!(
        decode_header(mpeg2).err(),
        Some(Mp3Error::Truncated)
    );
    // A valid header cut off after four bytes is a truncation, not a corrupt
    // frame: the decoder distinguishes "not enough bytes yet" from "garbage".
    assert_eq!(decode_header(valid).err(), Some(Mp3Error::Truncated));
}

#[test]
fn refuses_random_bytes_as_sync_loss() {
    let junk = [0x00u8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let mut decoder = Mp3Decoder::new();
    let mut pcm = vec![0i16; MAX_FRAME_SAMPLES * 2];
    decoder.push(&junk).unwrap();
    decoder.finish();
    assert_eq!(decoder.next_frame(&mut pcm).err(), Some(Mp3Error::SyncLost));
}

#[test]
fn a_low_bitrate_stream_uses_the_bit_reservoir_and_stays_stable() {
    let data = fixture("reservoir_44100_stereo_32k.mp3");
    let decoded = decode(&data).expect("a 32 kbps reservoir stream decodes");

    assert_eq!(decoded.sample_rate(), 44_100);
    assert_eq!(decoded.channels(), 2);
    assert!(decoded.frames >= 15, "{} frames", decoded.frames);

    // Proof that the decoder followed main_data_begin: a 32 kbps stream cannot
    // fit a granule in one frame, so LAME borrows bytes from earlier frames.
    let borrowing = decoded
        .info
        .iter()
        .filter(|info| info.main_data_begin > 0)
        .count();
    assert!(
        borrowing >= 3,
        "only {borrowing} frames borrowed main data; the fixture is not exercising the reservoir"
    );

    // Every active frame must carry the tone, and the level must stay steady:
    // a decoder that silently drops a reservoir frame would show a gap here.
    let levels = decoded.frame_rms(0, decoded.frames);
    assert!(levels.len() >= 8, "not enough active frames ({})", levels.len());
    let min = levels.iter().copied().fold(f32::INFINITY, f32::min);
    let max = levels.iter().copied().fold(0.0f32, f32::max);
    assert!(min > 0.1, "a reservoir frame decoded to near silence: {min}");
    assert!(
        min > max * 0.75,
        "frame level swings from {min} to {max}; reservoir handling is unstable"
    );
    assert_tone_level(decoded.rms(0, 0.25, 0.75), "32 kbps left channel");
}

#[test]
fn accepts_input_in_small_arbitrary_chunks() {
    // The filesystem chunk size belongs to the caller; the decoder must not
    // assume headers, side info and main data arrive together.
    let data = fixture("tone_44100_stereo_128k.mp3");
    let decoded = decode_with_chunk(&data, 37).expect("37-byte pushes decode");
    assert_eq!(decoded.sample_rate(), 44_100);
    assert!(decoded.frames >= 10);
}

#[test]
fn zz_debug_dump_pcm() {
    let temp_dir = std::env::temp_dir();
    for (name, out_name) in [
        ("tone_44100_mono_64k.mp3", "interos_ours_mono.raw"),
        ("tone_44100_stereo_128k.mp3", "interos_ours_stereo.raw"),
        ("reservoir_44100_stereo_32k.mp3", "interos_ours_res.raw"),
    ] {
        let data = fixture(name);
        let decoded = decode(&data).expect("decode");
        let mut bytes = Vec::with_capacity(decoded.pcm.len() * 2);
        for v in &decoded.pcm {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let out = temp_dir.join(out_name);
        let _ = std::fs::write(out, &bytes);
        let levels = decoded.frame_rms(0, decoded.frames);
        println!("{name}: frames={} pcm={} levels={:?}", decoded.frames, decoded.pcm.len(), levels);
    }
}
