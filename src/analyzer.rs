use crate::mod_file::{ModFile, ModSample, SampleUsage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderKind {
    Iff8Svx,
    RiffWave { unsigned_8bit: bool },
    CreativeVoc,
}

impl HeaderKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Iff8Svx => "Amiga IFF 8SVX",
            Self::RiffWave {
                unsigned_8bit: true,
            } => "RIFF WAVE (unsigned PCM)",
            Self::RiffWave {
                unsigned_8bit: false,
            } => "RIFF WAVE",
            Self::CreativeVoc => "Creative VOC",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SampleDefect {
    /// Raw container file header baked into the beginning of sample PCM
    EmbeddedHeader {
        kind: HeaderKind,
        strip_bytes: usize,
        summary: String,
    },
    /// Localized corruption burst (e.g. floppy/transfer sector glitch) inside active waveform
    MidSampleBurst {
        start: usize,
        end: usize,
        max_step: i32,
        ctx_p95_step: i32,
    },
    /// Pseudo-loop at sample end (tiny stop-loop or flat silent loop) on an effective one-shot
    PseudoOneShotLoop {
        repeat_start: usize,
        repeat_len: usize,
    },
    /// Garbage spike/burst occurring AFTER the sample already decayed to silence/DC floor
    PostSilenceBurst {
        truncate_to: usize,
        burst_start: usize,
        burst_end: usize,
        burst_max_step: i32,
    },
    /// Severe corruption burst in the already-decayed late tail (last 15%) of a one-shot
    LateTailBurst {
        truncate_to: usize,
        burst_start: usize,
        burst_end: usize,
        burst_max_step: i32,
    },
    /// Excessive dead silence or constant DC padding at the end of a one-shot
    TrailingDeadPadding {
        truncate_to: usize,
        dead_bytes: usize,
        dc_value: i8,
    },
    /// Abrupt non-zero amplitude cut-off at the end of a one-shot sample
    AbruptEndCliff {
        tail_peak: i32,
        fade_samples: usize,
    },
}

#[derive(Debug, Clone)]
pub struct SampleReport {
    pub index: usize,
    pub name: String,
    pub orig_len: usize,
    pub volume: u8,
    pub is_looped: bool,
    pub repeat_start: usize,
    pub repeat_len: usize,
    pub usage: SampleUsage,
    pub defects: Vec<SampleDefect>,
}

#[derive(Debug, Clone)]
pub struct ModuleReport {
    pub title: String,
    pub format_label: String,
    pub channels: usize,
    pub song_len: u8,
    pub num_patterns: usize,
    pub sample_reports: Vec<SampleReport>,
}

impl ModuleReport {
    pub fn total_defects(&self) -> usize {
        self.sample_reports.iter().map(|r| r.defects.len()).sum()
    }

    pub fn affected_samples(&self) -> usize {
        self.sample_reports
            .iter()
            .filter(|r| !r.defects.is_empty())
            .count()
    }
}

pub fn analyze_module(module: &ModFile) -> ModuleReport {
    let usages = module.analyze_sample_usage();
    let mut sample_reports = Vec::new();

    for smp in &module.samples {
        if smp.data.is_empty() {
            continue;
        }
        let usage = usages.get(smp.index).cloned().unwrap_or_default();
        let defects = analyze_sample(smp);
        sample_reports.push(SampleReport {
            index: smp.index,
            name: smp.name(),
            orig_len: smp.data.len(),
            volume: smp.volume,
            is_looped: smp.is_looped(),
            repeat_start: smp.repeat_start,
            repeat_len: smp.repeat_len,
            usage,
            defects,
        });
    }

    ModuleReport {
        title: module.title(),
        format_label: module.variant.magic_str(),
        channels: module.variant.num_channels(),
        song_len: module.song_len,
        num_patterns: module.num_patterns,
        sample_reports,
    }
}

pub fn analyze_sample(smp: &ModSample) -> Vec<SampleDefect> {
    let data = &smp.data;
    let len = data.len();
    if len < 16 {
        return Vec::new();
    }

    let mut defects = Vec::new();

    // 1. Check for embedded container headers (IFF 8SVX, RIFF WAVE, Creative VOC)
    let mut header_strip = 0usize;
    if let Some((kind, strip, summary)) = detect_embedded_header(data) {
        header_strip = strip.min(len);
        defects.push(SampleDefect::EmbeddedHeader {
            kind,
            strip_bytes: header_strip,
            summary,
        });
    }

    // 2. Determine if sample is a true One-Shot or Pseudo-One-Shot
    let (effective_oneshot, mut effective_end) = classify_oneshot_and_loop(smp, header_strip);
    if effective_oneshot && smp.is_looped() {
        defects.push(SampleDefect::PseudoOneShotLoop {
            repeat_start: smp.repeat_start,
            repeat_len: smp.repeat_len,
        });
    }

    // 3. Tail analysis for One-Shot samples (Post-silence bursts, late-tail corruption, dead padding)
    let mut tail_will_be_faded = false;
    if effective_oneshot && effective_end > header_strip + 64 {
        if let Some(post_sil) = detect_post_silence_burst(data, header_strip, effective_end) {
            effective_end = post_sil_truncate_target(&post_sil);
            tail_will_be_faded = true;
            defects.push(post_sil);
        } else if let Some(late_burst) = detect_late_tail_burst(data, header_strip, effective_end) {
            effective_end = late_burst_truncate_target(&late_burst);
            tail_will_be_faded = true;
            defects.push(late_burst);
        } else if let Some(pad) = detect_trailing_dead_padding(data, header_strip, effective_end) {
            effective_end = trailing_pad_truncate_target(&pad);
            tail_will_be_faded = true;
            defects.push(pad);
        }
    }

    // 4. Mid-sample localized sector/transfer corruption bursts in the remaining active body.
    //    For looped samples, only scan the one-shot attack region before `repeat_start` so we never
    //    alter a tuned sustain loop's wavelength/pitch.
    let burst_scan_end = if !effective_oneshot && smp.is_looped() {
        smp.repeat_start.min(effective_end)
    } else {
        effective_end
    };
    if burst_scan_end > header_strip + 320 {
        let bursts = detect_mid_sample_bursts(data, header_strip, burst_scan_end);
        for b in bursts {
            defects.push(b);
        }
    }

    // 5. Check if One-Shot ends with an abrupt non-zero amplitude cliff
    if effective_oneshot
        && !tail_will_be_faded
        && effective_end > header_strip + 32
        && let Some(cliff) = detect_abrupt_end_cliff(data, header_strip, effective_end)
    {
        defects.push(cliff);
    }

    defects
}

fn post_sil_truncate_target(d: &SampleDefect) -> usize {
    if let SampleDefect::PostSilenceBurst { truncate_to, .. } = d {
        *truncate_to
    } else {
        0
    }
}

fn late_burst_truncate_target(d: &SampleDefect) -> usize {
    if let SampleDefect::LateTailBurst { truncate_to, .. } = d {
        *truncate_to
    } else {
        0
    }
}

fn trailing_pad_truncate_target(d: &SampleDefect) -> usize {
    if let SampleDefect::TrailingDeadPadding { truncate_to, .. } = d {
        *truncate_to
    } else {
        0
    }
}

fn detect_embedded_header(data: &[i8]) -> Option<(HeaderKind, usize, String)> {
    let raw: Vec<u8> = data.iter().take(320).map(|&x| x as u8).collect();
    let scan_len = raw.len();

    // A) Amiga IFF 8SVX: Look for '8SVX', 'VHDR', 'ANNO', or 'BODY' within first 256 bytes
    let has_8svx_marker = find_subslice(&raw[..scan_len.min(64)], b"8SVX").is_some()
        || find_subslice(&raw[..scan_len.min(64)], b"VHDR").is_some();
    let mut last_body_pcm_start: Option<(usize, u32)> = None;

    let search_limit = scan_len.min(256);
    if search_limit >= 12 {
        for pos in 0..=(search_limit - 8) {
            if &raw[pos..pos + 4] == b"BODY" {
                let body_len =
                    u32::from_be_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]]);
                // Validate plausible 8SVX BODY length (within reasonable factor of sample length)
                let min_expected = (data.len() / 4) as u32;
                let max_expected = (data.len() as u32).saturating_add(8192).max(256);
                if (has_8svx_marker && (16..=524_288).contains(&body_len))
                    || (pos <= 32 && body_len >= min_expected && body_len <= max_expected)
                {
                    last_body_pcm_start = Some((pos + 8, body_len));
                }
            }
        }
    }

    if let Some((pcm_start, body_len)) = last_body_pcm_start {
        let aligned = (pcm_start + 1) & !1;
        if aligned < data.len().saturating_sub(8) {
            let summary = if has_8svx_marker {
                format!("IFF 8SVX header + chunks (BODY len={body_len}), PCM starts at +{aligned}")
            } else {
                format!(
                    "Orphaned IFF BODY chunk header (len={body_len}), PCM starts at +{aligned}"
                )
            };
            return Some((HeaderKind::Iff8Svx, aligned, summary));
        }
    }

    // B) RIFF WAVE ('WAVEfmt ' ... 'data')
    if let Some(wave_pos) = find_subslice(&raw[..scan_len.min(64)], b"WAVEfmt ")
        && let Some(rel_data) = find_subslice(&raw[wave_pos..scan_len.min(256)], b"data")
    {
        let pcm_start = wave_pos + rel_data + 8;
        let aligned = (pcm_start + 1) & !1;
        if aligned < data.len().saturating_sub(16) {
            // Heuristic check for unsigned 8-bit WAV PCM (silence sits around 0x80 = -128 i8)
            let tail_slice = &raw[aligned..raw.len()];
            let near_128 = tail_slice
                .iter()
                .filter(|&&b| (120..=136).contains(&b))
                .count();
            let unsigned_8bit = tail_slice.len() >= 32 && near_128 * 2 > tail_slice.len();
            return Some((
                HeaderKind::RiffWave { unsigned_8bit },
                aligned,
                format!("RIFF WAVE header, PCM starts at +{aligned}"),
            ));
        }
    }

    // C) Creative Voice File
    if find_subslice(&raw[..scan_len.min(32)], b"Creative Voice File").is_some()
        && data.len() > 64
    {
        let offset = if raw.len() >= 22 {
            u16::from_le_bytes([raw[20], raw[21]]) as usize
        } else {
            26
        };
        let aligned = ((offset + 4).clamp(26, 64) + 1) & !1;
        return Some((
            HeaderKind::CreativeVoc,
            aligned,
            format!("Creative VOC header, PCM starts at +{aligned}"),
        ));
    }

    None
}

fn classify_oneshot_and_loop(smp: &ModSample, start: usize) -> (bool, usize) {
    let len = smp.data.len();
    if !smp.is_looped() {
        return (true, len);
    }

    let loop_start = smp.repeat_start.max(start).min(len);
    let loop_end = (smp.repeat_start + smp.repeat_len).min(len);
    let loop_len = loop_end.saturating_sub(loop_start);
    let pre_loop = loop_start.saturating_sub(start);

    // Classic 2-4 word Amiga stop loop at the end of a long sample
    if loop_len <= 8 && pre_loop >= 64 && loop_end + 8 >= len {
        return (true, loop_start);
    }

    // Flat/silent end-loop -> acoustically a One-Shot
    if loop_len > 0 && pre_loop >= 32 {
        let mut min_v = i32::MAX;
        let mut max_v = i32::MIN;
        for &v in &smp.data[loop_start..loop_end] {
            let vi = v as i32;
            min_v = min_v.min(vi);
            max_v = max_v.max(vi);
        }
        if max_v - min_v <= 4 && max_v.abs().max(min_v.abs()) <= 4 {
            return (true, loop_start);
        }
    }

    (false, len)
}

/// Detects a garbage burst that appears AFTER the sample has already decayed to silence/DC
/// (e.g., KILLBASS Sample 3 `st-06:snare` decaying to 0/-1 at +2496..+2647, then exploding
/// to +/-128 at +2648..+2671).
fn detect_post_silence_burst(data: &[i8], start: usize, end: usize) -> Option<SampleDefect> {
    let body_len = end.saturating_sub(start);
    if body_len < 192 {
        return None;
    }

    // Find global peak to know where the main body is
    let mut peak_abs = 0i32;
    for &v in &data[start..end] {
        peak_abs = peak_abs.max((v as i32).abs());
    }
    if peak_abs < 24 {
        return None;
    }

    // Only look in the second half of the sample (after main transient/body)
    let search_from = start + body_len / 2;
    let quiet_win = 48usize;
    if search_from + quiet_win + 8 >= end {
        return None;
    }

    let mut idx = search_from;
    while idx + quiet_win < end {
        let win = &data[idx..idx + quiet_win];
        let mut min_v = i32::MAX;
        let mut max_v = i32::MIN;
        for &v in win {
            let vi = v as i32;
            min_v = min_v.min(vi);
            max_v = max_v.max(vi);
        }

        // Quiet / constant DC floor: peak-to-peak <= 4 and |value| <= 4
        if max_v - min_v <= 4 && max_v.abs().max(min_v.abs()) <= 4 {
            let dc_ref = (min_v + max_v) / 2;
            // Extend quiet zone forward until signal breaks out of the quiet floor
            let mut q_end = idx + quiet_win;
            while q_end < end {
                let v = data[q_end] as i32;
                if (v - dc_ref).abs() > 4 {
                    break;
                }
                q_end += 1;
            }

            // Inspect everything after q_end up to `end`
            if q_end < end {
                let mut burst_max_step = 0i32;
                let mut burst_max_dev = 0i32;
                let mut burst_end = q_end;
                for p in q_end..end {
                    let v = data[p] as i32;
                    let prev = data[p - 1] as i32;
                    let st = (v - prev).abs();
                    let dev = (v - dc_ref).abs();
                    if dev > 4 {
                        burst_end = p;
                    }
                    burst_max_step = burst_max_step.max(st);
                    burst_max_dev = burst_max_dev.max(dev);
                }

                // Make sure there isn't a second legitimate musical hit (i.e. burst is short <= 128 samples)
                let active_after = burst_end.saturating_sub(q_end) + 1;
                if (burst_max_step >= 20 || burst_max_dev >= 12) && active_after <= 128 {
                    let keep = (idx + 16).min(q_end);
                    let truncate_to = (keep + 1) & !1;
                    return Some(SampleDefect::PostSilenceBurst {
                        truncate_to,
                        burst_start: q_end,
                        burst_end,
                        burst_max_step,
                    });
                }
            }
            break;
        }
        idx += 4;
    }

    None
}

/// Detects severe corruption bursts in the final 15% of an already-decayed One-Shot sample
/// (e.g., KILLBASS Sample 2 `st-06:mixfx2` at +18800..+18877 out of 20300).
fn detect_late_tail_burst(data: &[i8], start: usize, end: usize) -> Option<SampleDefect> {
    let body_len = end.saturating_sub(start);
    if body_len < 1024 {
        return None;
    }

    // Compute peak RMS in the main body
    let mut peak_avg_abs = 0.0f64;
    let blk = 256usize;
    let mut b0 = start;
    while b0 + blk <= end {
        let sum: i32 = data[b0..b0 + blk].iter().map(|&v| (v as i32).abs()).sum();
        let avg = (sum as f64) / (blk as f64);
        if avg > peak_avg_abs {
            peak_avg_abs = avg;
        }
        b0 += blk;
    }
    if peak_avg_abs < 20.0 {
        return None;
    }

    // Inspect the last 15% of the sample
    let tail_zone_start = end - (body_len * 15) / 100;
    if tail_zone_start <= start + 256 {
        return None;
    }

    for i in tail_zone_start..end {
        let step = ((data[i] as i32) - (data[i - 1] as i32)).abs();
        if step < 110 {
            continue;
        }

        // Measure clean preceding window [i - 192 .. i - 16]
        let pre_a = i.saturating_sub(192).max(start + 1);
        let pre_b = i.saturating_sub(16).max(pre_a + 1);
        if pre_b <= pre_a + 32 {
            continue;
        }

        let mut pre_max_step = 0i32;
        let mut pre_sum_abs = 0i32;
        for k in pre_a..pre_b {
            let st = ((data[k] as i32) - (data[k - 1] as i32)).abs();
            pre_max_step = pre_max_step.max(st);
            pre_sum_abs += (data[k] as i32).abs();
        }
        let pre_avg_abs = (pre_sum_abs as f64) / ((pre_b - pre_a) as f64);

        // If the signal had already decayed (< 55% of peak body average) and suddenly jumps > 2.8x pre_max_step
        if pre_avg_abs < peak_avg_abs * 0.55 && step > (pre_max_step * 28) / 10 && pre_max_step <= 65
        {
            // Walk backwards across any elevated shoulder steps so we cut cleanly before the glitch
            let mut b_start = i;
            while b_start > i.saturating_sub(32) && b_start > start + 16 {
                let st_prev = ((data[b_start - 1] as i32) - (data[b_start - 2] as i32)).abs();
                if st_prev <= pre_max_step {
                    // Check one more step back in case a single quiet step sits inside the shoulder
                    let st_prev2 = if b_start >= 3 {
                        ((data[b_start - 2] as i32) - (data[b_start - 3] as i32)).abs()
                    } else {
                        0
                    };
                    if st_prev2 <= pre_max_step {
                        break;
                    }
                }
                b_start -= 1;
            }
            let burst_start = b_start.saturating_sub(4);
            let mut burst_end = i;
            let mut burst_max_step = step;
            for k in i..(end.min(i + 256)) {
                let st = ((data[k] as i32) - (data[k - 1] as i32)).abs();
                if st > pre_max_step * 2 {
                    burst_end = k;
                    burst_max_step = burst_max_step.max(st);
                }
            }
            let truncate_to = burst_start & !1;
            if truncate_to > start + 256 {
                return Some(SampleDefect::LateTailBurst {
                    truncate_to,
                    burst_start,
                    burst_end,
                    burst_max_step,
                });
            }
        }
    }

    None
}

fn detect_trailing_dead_padding(data: &[i8], start: usize, end: usize) -> Option<SampleDefect> {
    let body_len = end.saturating_sub(start);
    if body_len < 160 {
        return None;
    }

    // Reference DC from last few bytes (skipping the last 4 zeroed word-padding bytes if present)
    let probe = end.saturating_sub(6);
    let dc_ref = data[probe] as i32;
    if dc_ref.abs() > 4 {
        return None;
    }

    let mut p = end;
    while p > start + 32 {
        let v = data[p - 1] as i32;
        if v != 0 && (v - dc_ref).abs() > 1 {
            break;
        }
        p -= 1;
    }

    let dead_bytes = end.saturating_sub(p);
    // Only flag dead padding by default if it carries a noticeable DC offset (|dc| >= 3)
    // or wastes >= 2048 bytes at the end of a sample.
    if (dc_ref.abs() >= 3 && dead_bytes >= 64) || dead_bytes >= 2048 {
        let truncate_to = ((p + 16).min(end) + 1) & !1;
        if truncate_to + 64 <= end {
            return Some(SampleDefect::TrailingDeadPadding {
                truncate_to,
                dead_bytes,
                dc_value: dc_ref as i8,
            });
        }
    }

    None
}

/// Detects localized mid-sample corruption islands (e.g. 4KB sector boundary glitches at
/// +6550..+6588, +10644..+10682, +14764..+14780 in KILLBASS Sample 2).
fn detect_mid_sample_bursts(data: &[i8], start: usize, end: usize) -> Vec<SampleDefect> {
    let mut results = Vec::new();
    // Skip initial attack transient zone (first 256 bytes after header)
    let scan_start = start + 256;
    let scan_end = end.saturating_sub(32);
    if scan_start + 64 >= scan_end {
        return results;
    }

    let mut seeds: Vec<usize> = Vec::new();
    for i in scan_start..scan_end {
        let d0 = (data[i] as i32) - (data[i - 1] as i32);
        let d1 = (data[i + 1] as i32) - (data[i] as i32);
        let ad0 = d0.abs();
        let ad1 = d1.abs();

        // Fast pre-filter: must have at least one large step >= 115
        if ad0 < 115 && ad1 < 115 {
            continue;
        }

        // Check for instantaneous polarity whiplash or 4-sample multi-wrap
        let whiplash_2 = (d0 * d1 < 0) && ad0.max(ad1) >= 125 && ad0.min(ad1) >= 55;
        let mut big_pos = 0;
        let mut big_neg = 0;
        let win_hi = (i + 8).min(end - 1);
        for k in i.saturating_sub(2)..=win_hi {
            let dk = (data[k] as i32) - (data[k - 1] as i32);
            if dk >= 115 {
                big_pos += 1;
            } else if dk <= -115 {
                big_neg += 1;
            }
        }
        let multi_wrap = big_pos >= 1 && big_neg >= 1 && (ad0 >= 145 || (big_pos + big_neg >= 3));

        if !whiplash_2 && !multi_wrap && ad0 < 165 {
            continue;
        }

        // Compare against clean surrounding neighborhood [i-220..i-48] and [i+48..i+220]
        let ctx_stats = local_clean_step_stats(data, start, end, i);
        let (ctx_p95, ctx_max, ctx_whiplashes) = ctx_stats;

        // If surrounding waveform itself has constant whiplashes (e.g. white noise cymbal), skip!
        if ctx_whiplashes >= 2 || ctx_p95 > 95 {
            continue;
        }

        let is_corrupt = (whiplash_2 && ad0.max(ad1) > ctx_p95 * 2)
            || (multi_wrap && ad0.max(ad1) > (ctx_p95 * 22) / 10)
            || (ad0 >= 160 && ad0 > (ctx_max * 16) / 10);

        if is_corrupt {
            seeds.push(i);
        }
    }

    if seeds.is_empty() {
        return results;
    }

    // Merge seeds within 48 samples into contiguous corruption islands
    let mut idx = 0;
    while idx < seeds.len() {
        let first = seeds[idx];
        let mut last = first;
        while idx + 1 < seeds.len() && seeds[idx + 1] <= last + 48 {
            idx += 1;
            last = seeds[idx];
        }
        idx += 1;

        let (ctx_p95, _, _) = local_clean_step_stats(data, start, end, (first + last) / 2);

        // Expand slightly to capture shoulder glitches (steps > ctx_p95 * 1.6)
        let mut b_start = first.saturating_sub(6).max(start + 16);
        while b_start > first.saturating_sub(24) && b_start > start + 16 {
            let st = ((data[b_start] as i32) - (data[b_start - 1] as i32)).abs();
            if st <= (ctx_p95 * 15) / 10 {
                break;
            }
            b_start -= 1;
        }

        let mut b_end = (last + 12).min(end - 4);
        while b_end < (last + 28).min(end - 4) {
            let st = ((data[b_end + 1] as i32) - (data[b_end] as i32)).abs();
            if st <= (ctx_p95 * 13) / 10 {
                break;
            }
            b_end += 1;
        }

        let width = b_end.saturating_sub(b_start) + 1;
        if width <= 160 {
            let mut max_step = 0i32;
            let mut pos_big = 0usize;
            let mut neg_big = 0usize;
            for k in b_start..=b_end {
                let dk = (data[k] as i32) - (data[k - 1] as i32);
                max_step = max_step.max(dk.abs());
                if dk >= 100 {
                    pos_big += 1;
                } else if dk <= -100 {
                    neg_big += 1;
                }
            }

            // Distinguish true multi-byte sector/transfer bursts (or huge spikes in smooth pads)
            // from single-sample sawtooth flybacks or single-peak clipping in bright synths.
            let is_smooth_pad_spike = ctx_p95 <= 30 && max_step >= 130 && max_step > ctx_p95 * 4;
            let is_multi_glitch_cluster = (last.saturating_sub(first) >= 4
                || (pos_big + neg_big >= 3 && pos_big >= 1 && neg_big >= 1))
                && max_step > (ctx_p95 * 22) / 10;

            if is_smooth_pad_spike || is_multi_glitch_cluster {
                results.push(SampleDefect::MidSampleBurst {
                    start: b_start,
                    end: b_end,
                    max_step,
                    ctx_p95_step: ctx_p95,
                });
            }
        }
    }

    results
}

fn local_clean_step_stats(
    data: &[i8],
    start: usize,
    end: usize,
    center: usize,
) -> (i32, i32, usize) {
    let mut steps = Vec::with_capacity(280);
    let mut whiplashes = 0usize;

    let left_a = center.saturating_sub(220).max(start + 1);
    let left_b = center.saturating_sub(48).max(left_a);
    let right_a = (center + 48).min(end.saturating_sub(2));
    let right_b = (center + 220).min(end.saturating_sub(2));

    for range in [left_a..left_b, right_a..right_b] {
        for k in range {
            let d0 = (data[k] as i32) - (data[k - 1] as i32);
            let d1 = (data[k + 1] as i32) - (data[k] as i32);
            steps.push(d0.abs());
            if d0 * d1 < 0 && d0.abs().max(d1.abs()) >= 125 && d0.abs().min(d1.abs()) >= 55 {
                whiplashes += 1;
            }
        }
    }

    if steps.is_empty() {
        return (32, 64, 0);
    }
    steps.sort_unstable();
    let p95 = steps[(steps.len() * 95 / 100).min(steps.len() - 1)].max(8);
    let p99 = steps[(steps.len() * 99 / 100).min(steps.len() - 1)].max(p95);
    (p95, p99, whiplashes)
}

fn detect_abrupt_end_cliff(data: &[i8], start: usize, end: usize) -> Option<SampleDefect> {
    if end <= start + 32 {
        return None;
    }
    // Skip trailing exact zeros (up to 6 bytes of Amiga word padding)
    let mut p = end;
    let mut skipped_zeros = 0;
    while p > start + 16 && data[p - 1] == 0 && skipped_zeros < 6 {
        p -= 1;
        skipped_zeros += 1;
    }
    if p <= start + 16 {
        return None;
    }

    // Look at the final 2 non-zero samples right at the cutoff cliff
    let last_val = (data[p - 1] as i32).abs();
    let prev_val = (data[p - 2] as i32).abs();
    let tail_peak = last_val.max(prev_val);

    if last_val >= 28 || tail_peak >= 36 {
        let fade_samples = (end - start).clamp(16, 48);
        return Some(SampleDefect::AbruptEndCliff {
            tail_peak,
            fade_samples,
        });
    }

    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}
