use std::f64::consts::PI;

use mod_fixer::analyzer::{HeaderKind, SampleDefect, analyze_module, analyze_sample};
use mod_fixer::fixer::{FixOptions, fix_module, fix_sample};
use mod_fixer::mod_file::{ModFile, ModSample, ModVariant};

fn make_sine_sample(index: usize, name: &str, len: usize, amp: f64, period: f64) -> ModSample {
    let mut raw_name = [0u8; 22];
    let bytes = name.as_bytes();
    let copy_len = bytes.len().min(22);
    raw_name[..copy_len].copy_from_slice(&bytes[..copy_len]);

    let mut data = Vec::with_capacity(len);
    for i in 0..len {
        let env = 1.0 - (i as f64 / len as f64);
        let val = (amp * env * (2.0 * PI * (i as f64) / period).sin()).round() as i8;
        data.push(val);
    }

    ModSample {
        index,
        raw_name,
        finetune: 0,
        volume: 64,
        repeat_start: 0,
        repeat_len: 2,
        data,
    }
}

#[test]
fn strips_embedded_iff_8svx_header_and_preserves_audio() {
    let mut smp = make_sine_sample(1, "st-06:test", 1000, 90.0, 40.0);
    let mut header = Vec::new();
    header.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x04, 0x30]);
    header.extend_from_slice(b"8SVXVHDR");
    header.extend_from_slice(&20u32.to_be_bytes());
    header.extend_from_slice(&[0u8; 20]);
    header.extend_from_slice(b"BODY");
    header.extend_from_slice(&(smp.data.len() as u32).to_be_bytes());
    assert_eq!(header.len(), 48);

    let mut prefixed: Vec<i8> = header.into_iter().map(|b| b as i8).collect();
    prefixed.extend_from_slice(&smp.data);
    smp.data = prefixed;

    let defects = analyze_sample(&smp);
    assert!(defects.iter().any(|d| matches!(
        d,
        SampleDefect::EmbeddedHeader {
            kind: HeaderKind::Iff8Svx,
            strip_bytes: 48,
            ..
        }
    )));

    let actions = fix_sample(&mut smp, FixOptions::default());
    assert!(!actions.is_empty());
    assert_eq!(smp.data.len(), 1000);
    assert_eq!(smp.data[0], 0);
    assert_eq!(smp.data[1], 0);
    assert!(analyze_sample(&smp).is_empty());
}

#[test]
fn truncates_post_silence_tail_burst() {
    let mut smp = make_sine_sample(3, "st-06:snare", 2000, 110.0, 24.0);
    // Append 160 samples of quiet floor (-1 / 0)
    smp.data.extend((0..160).map(|i| if i % 2 == 0 { 0 } else { -1 }));
    // Append 24 samples of full-scale garbage burst
    smp.data.extend_from_slice(&[
        -32, 63, -64, 123, -123, -12, 5, -34, 60, -128, 3, 121, 56, -116, 33, -117,
    ]);
    // Append 64 samples of -2 DC tail
    smp.data.extend(std::iter::repeat_n(-2i8, 64));

    let orig_len = smp.data.len();
    let defects = analyze_sample(&smp);
    assert!(
        defects
            .iter()
            .any(|d| matches!(d, SampleDefect::PostSilenceBurst { .. }))
    );

    fix_sample(&mut smp, FixOptions::default());
    assert!(smp.data.len() < orig_len - 80);
    assert_eq!(smp.data[smp.data.len() - 1], 0);
    assert!(analyze_sample(&smp).is_empty());
}

#[test]
fn splices_mid_sample_sector_burst_cleanly_and_is_idempotent() {
    let mut smp = make_sine_sample(2, "st-06:mixfx2", 4096, 85.0, 48.0);
    // Inject a 24-byte sector corruption burst around index 2000
    let glitch: [i8; 16] = [
        -128, 103, 9, 81, -51, 97, -128, 0, 73, 124, -24, 100, -103, 33, -126, 60,
    ];
    smp.data[2000..2016].copy_from_slice(&glitch);

    let defects = analyze_sample(&smp);
    assert!(
        defects
            .iter()
            .any(|d| matches!(d, SampleDefect::MidSampleBurst { .. }))
    );

    fix_sample(&mut smp, FixOptions::default());
    assert_eq!(smp.data.len() % 2, 0);
    assert!(
        analyze_sample(&smp).is_empty(),
        "repaired sample should have zero remaining defects"
    );

    // Verify idempotency on second pass
    let len_after_first = smp.data.len();
    let second_actions = fix_sample(&mut smp, FixOptions::default());
    assert!(second_actions.is_empty());
    assert_eq!(smp.data.len(), len_after_first);
}

#[test]
fn does_not_touch_tuned_sustain_loop_wavelength() {
    let mut smp = make_sine_sample(1, "tuned_saw", 2000, 100.0, 32.0);
    smp.repeat_start = 400;
    smp.repeat_len = 1600;
    // Steep sawtooth-like jump inside sustain loop must never detune repeat_len
    smp.data[1000] = 90;
    smp.data[1001] = -90;

    let actions = fix_sample(&mut smp, FixOptions::default());
    assert!(actions.is_empty());
    assert_eq!(smp.repeat_start, 400);
    assert_eq!(smp.repeat_len, 1600);
}

#[test]
fn roundtrips_protracker_mod_binary() {
    let mut samples = Vec::with_capacity(31);
    for idx in 1..=31 {
        if idx == 1 {
            let mut s = make_sine_sample(1, "clean_pad", 512, 64.0, 32.0);
            // Append post-silence spike
            s.data.extend(std::iter::repeat_n(0i8, 80));
            s.data.extend_from_slice(&[120, -120, 110, -115, 0, 0, 0, 0]);
            samples.push(s);
        } else {
            samples.push(ModSample {
                index: idx,
                raw_name: [0u8; 22],
                finetune: 0,
                volume: 0,
                repeat_start: 0,
                repeat_len: 2,
                data: Vec::new(),
            });
        }
    }

    let mut raw_title = [0u8; 20];
    raw_title[..8].copy_from_slice(b"TestSong");
    let orders = [0u8; 128];
    let pattern_data = vec![0u8; 1024];

    let orig = ModFile {
        raw_title,
        variant: ModVariant::ProTracker31 {
            magic: *b"M.K.",
            channels: 4,
        },
        samples,
        song_len: 1,
        restart_pos: 127,
        orders,
        num_patterns: 1,
        pattern_data,
        trailing_bytes: 0,
    };

    let raw = orig.to_bytes().expect("serialize");
    let mut parsed = ModFile::parse(&raw).expect("parse");
    assert_eq!(parsed.title(), "TestSong");
    assert!(analyze_module(&parsed).total_defects() >= 1);

    let summary = fix_module(&mut parsed, FixOptions::default());
    assert_eq!(summary.sample_actions.len(), 1);
    let fixed_bytes = parsed.to_bytes().expect("serialize fixed");
    let reparsed = ModFile::parse(&fixed_bytes).expect("reparse fixed");
    assert_eq!(analyze_module(&reparsed).total_defects(), 0);
}
