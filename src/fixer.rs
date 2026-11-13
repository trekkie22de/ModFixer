use std::f64::consts::PI;

use crate::analyzer::{HeaderKind, SampleDefect, analyze_sample};
use crate::mod_file::{ModFile, ModSample};

#[derive(Debug, Clone, Copy)]
pub struct FixOptions {
    pub strip_headers: bool,
    pub trim_tails: bool,
    pub splice_bursts: bool,
    pub fade_cliffs: bool,
}

impl Default for FixOptions {
    fn default() -> Self {
        Self {
            strip_headers: true,
            trim_tails: true,
            splice_bursts: true,
            fade_cliffs: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SampleFixAction {
    pub sample_index: usize,
    pub sample_name: String,
    pub orig_len: usize,
    pub new_len: usize,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FixSummary {
    pub sample_actions: Vec<SampleFixAction>,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

pub fn fix_module(module: &mut ModFile, opts: FixOptions) -> FixSummary {
    let bytes_before: usize = module.samples.iter().map(|s| s.data.len()).sum();
    let mut sample_actions = Vec::new();

    for smp in &mut module.samples {
        if smp.data.is_empty() {
            continue;
        }
        let orig_len = smp.data.len();
        let actions = fix_sample(smp, opts);
        if !actions.is_empty() {
            sample_actions.push(SampleFixAction {
                sample_index: smp.index,
                sample_name: smp.name(),
                orig_len,
                new_len: smp.data.len(),
                actions,
            });
        }
    }

    let bytes_after: usize = module.samples.iter().map(|s| s.data.len()).sum();
    FixSummary {
        sample_actions,
        bytes_before,
        bytes_after,
    }
}

pub fn fix_sample(smp: &mut ModSample, opts: FixOptions) -> Vec<String> {
    let defects = analyze_sample(smp);
    if defects.is_empty() {
        return Vec::new();
    }

    let mut log = Vec::new();
    let mut did_tail_fade = false;

    // 1. Pseudo-One-Shot loop normalization & Tail Truncation (operates at high indices first)
    if opts.trim_tails {
        for d in &defects {
            if let SampleDefect::PseudoOneShotLoop {
                repeat_start,
                repeat_len,
            } = d
            {
                let cut = (*repeat_start).max(4) & !1;
                if cut < smp.data.len() {
                    smp.data.truncate(cut);
                }
                smp.repeat_start = 0;
                smp.repeat_len = 2;
                log.push(format!(
                    "normalized pseudo-loop ({repeat_start}+{repeat_len}) -> one-shot (len={})",
                    smp.data.len()
                ));
            }
        }

        for d in &defects {
            match d {
                SampleDefect::PostSilenceBurst {
                    truncate_to,
                    burst_start,
                    burst_end,
                    burst_max_step,
                } => {
                    let target = (*truncate_to).min(smp.data.len()) & !1;
                    if target >= 8 && target < smp.data.len() {
                        let old = smp.data.len();
                        smp.data.truncate(target);
                        apply_cosine_fade_out(&mut smp.data, 16);
                        smp.repeat_start = 0;
                        smp.repeat_len = 2;
                        did_tail_fade = true;
                        log.push(format!(
                            "truncated post-silence burst @{burst_start}..{burst_end} (maxStep={burst_max_step}): {old} -> {target} B"
                        ));
                    }
                }
                SampleDefect::LateTailBurst {
                    truncate_to,
                    burst_start,
                    burst_end,
                    burst_max_step,
                } => {
                    let target = (*truncate_to).min(smp.data.len()) & !1;
                    if target >= 64 && target < smp.data.len() {
                        let old = smp.data.len();
                        smp.data.truncate(target);
                        let fade_len = (target / 8).clamp(32, 128);
                        apply_cosine_fade_out(&mut smp.data, fade_len);
                        smp.repeat_start = 0;
                        smp.repeat_len = 2;
                        did_tail_fade = true;
                        log.push(format!(
                            "truncated late-tail burst @{burst_start}..{burst_end} (maxStep={burst_max_step}): {old} -> {target} B ({fade_len}-sample fade)"
                        ));
                    }
                }
                SampleDefect::TrailingDeadPadding {
                    truncate_to,
                    dead_bytes,
                    dc_value,
                } => {
                    let target = (*truncate_to).min(smp.data.len()) & !1;
                    if target >= 8 && target < smp.data.len() {
                        let old = smp.data.len();
                        smp.data.truncate(target);
                        apply_cosine_fade_out(&mut smp.data, 12);
                        smp.repeat_start = 0;
                        smp.repeat_len = 2;
                        did_tail_fade = true;
                        log.push(format!(
                            "trimmed {dead_bytes} B dead tail (DC={dc_value}): {old} -> {target} B"
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // 2. Mid-sample corruption bursts (process strictly right-to-left so earlier indices stay valid)
    if opts.splice_bursts {
        let mut bursts: Vec<(usize, usize, i32)> = defects
            .iter()
            .filter_map(|d| match d {
                SampleDefect::MidSampleBurst {
                    start,
                    end,
                    max_step,
                    ..
                } => Some((*start, *end, *max_step)),
                _ => None,
            })
            .collect();
        bursts.sort_by_key(|a| std::cmp::Reverse(a.0));

        for (b_start, b_end, max_step) in bursts {
            if b_end >= smp.data.len().saturating_sub(8) {
                continue;
            }
            if let Some((cut_l, cut_r)) = splice_corruption_island(smp, b_start, b_end) {
                let removed = cut_r - cut_l;
                log.push(format!(
                    "spliced mid-sample burst @{b_start}..{b_end} (maxStep={max_step}) via phase-matched cut @{cut_l}..{cut_r} (-{removed} B)"
                ));
            }
        }
    }

    // 3. Abrupt end cliff fade-out (if tail wasn't already truncated & faded above)
    if opts.fade_cliffs && !did_tail_fade && !smp.is_looped() {
        for d in &defects {
            if let SampleDefect::AbruptEndCliff {
                tail_peak,
                fade_samples,
            } = d
            {
                let fl = (*fade_samples).min(smp.data.len() / 2);
                if fl >= 8 {
                    apply_cosine_fade_out(&mut smp.data, fl);
                    log.push(format!(
                        "smoothed abrupt end cliff (peak={tail_peak}) with {fl}-sample fade-out"
                    ));
                }
            }
        }
    }

    // 4. Embedded Container Header stripping at index 0 (done last so all offsets above matched analysis!)
    if opts.strip_headers {
        for d in &defects {
            if let SampleDefect::EmbeddedHeader {
                kind, strip_bytes, ..
            } = d
            {
                let strip = (*strip_bytes).min(smp.data.len().saturating_sub(4)) & !1;
                if strip >= 2 {
                    smp.data.drain(0..strip);
                    if matches!(
                        kind,
                        HeaderKind::RiffWave {
                            unsigned_8bit: true
                        }
                    ) {
                        for v in &mut smp.data {
                            *v = ((*v as u8).wrapping_sub(128)) as i8;
                        }
                    }

                    if smp.is_looped() {
                        if smp.repeat_start >= strip {
                            smp.repeat_start -= strip;
                        } else {
                            let overlap = strip - smp.repeat_start;
                            smp.repeat_start = 0;
                            smp.repeat_len = smp.repeat_len.saturating_sub(overlap).max(4) & !1;
                        }
                    } else if smp.data.len() >= 8 {
                        // ProTracker / Amiga Paula convention: first word 0x0000 + short click-free attack ramp
                        smp.data[0] = 0;
                        smp.data[1] = 0;
                        let ramp = 6.min(smp.data.len() - 2);
                        for k in 0..ramp {
                            let w = (k + 1) as f64 / (ramp + 1) as f64;
                            smp.data[2 + k] = ((smp.data[2 + k] as f64) * w).round() as i8;
                        }
                    }

                    log.push(format!(
                        "stripped {} (-{strip} B at sample start, new len={} B)",
                        kind.label(),
                        smp.data.len()
                    ));
                }
            }
        }
    }

    // Ensure final length is even (Amiga 16-bit word aligned)
    if !smp.data.len().is_multiple_of(2) {
        smp.data.push(0);
    }

    log
}

/// Finds an even-length (`(r - l).is_multiple_of(2)`) phase- and slope-matched cut across `[b_start..=b_end]`,
/// applies a short pre-seam blend for C1 continuity, removes `l..r`, and updates loop pointers.
fn splice_corruption_island(
    smp: &mut ModSample,
    b_start: usize,
    b_end: usize,
) -> Option<(usize, usize)> {
    let data = &mut smp.data;
    let len = data.len();
    if b_start < 8 || b_end + 8 >= len || b_start > b_end {
        return None;
    }

    let search_left_min = b_start.saturating_sub(80).max(4);
    let search_left_max = b_start;
    let search_right_min = (b_end + 1).min(len - 4);
    let search_right_max = (b_end + 81).min(len - 4);

    let mut best_pair: Option<(usize, usize, i64)> = None;

    for l in search_left_min..=search_left_max {
        let vl_prev = data[l - 1] as i64;
        let vl_slope = (data[l - 1] as i64) - (data[l - 2] as i64);
        let l_zero_cross = (data[l - 2] as i32) * (data[l - 1] as i32) <= 0;

        for r in search_right_min..=search_right_max {
            // Keep cut strictly word-aligned (even byte count)
            if !(r - l).is_multiple_of(2) {
                continue;
            }
            let vr_cur = data[r] as i64;
            let vr_slope = (data[r + 1] as i64) - (data[r] as i64);

            // Continuity error between data[l-1] -> data[r] compared to expected slope
            let predicted_r = vl_prev + vl_slope;
            let val_err = (vr_cur - predicted_r).abs();
            let slope_err = (vr_slope - vl_slope).abs();
            let sign_mismatch = if vl_slope * vr_slope < 0 { 40 } else { 0 };
            let zc_bonus = if l_zero_cross && vr_cur.abs() <= 12 {
                -15
            } else {
                0
            };
            let dist_pen = ((b_start - l) + (r - (b_end + 1))) as i64 / 4;

            let cost = val_err * 6 + slope_err * 3 + sign_mismatch + dist_pen + zc_bonus;
            if best_pair.is_none_or(|(_, _, best_c)| cost < best_c) {
                best_pair = Some((l, r, cost));
            }
        }
    }

    let (cut_l, cut_r, _) = best_pair?;
    if cut_r <= cut_l {
        return None;
    }

    // Apply 6-sample C1 overlap blend before draining `cut_l..cut_r` if pre-cut right shoulder is clean
    let blend = 6usize.min(cut_l).min(cut_r.saturating_sub(b_end + 1));
    if blend >= 2 {
        for k in 0..blend {
            let t = (k + 1) as f64 / (blend + 1) as f64;
            let w_right = 0.5 - 0.5 * (PI * t).cos();
            let lhs = data[cut_l - blend + k] as f64;
            let rhs = data[cut_r - blend + k] as f64;
            data[cut_l - blend + k] = (lhs * (1.0 - w_right) + rhs * w_right).round() as i8;
        }
    }

    let removed = cut_r - cut_l;
    data.drain(cut_l..cut_r);

    // Adjust loop pointers if sample is looped
    if smp.is_looped() {
        let loop_end = smp.repeat_start + smp.repeat_len;
        if cut_r <= smp.repeat_start {
            smp.repeat_start -= removed;
        } else if cut_l >= smp.repeat_start && cut_r <= loop_end {
            smp.repeat_len = smp.repeat_len.saturating_sub(removed).max(4) & !1;
        } else if cut_l < smp.repeat_start && cut_r > smp.repeat_start {
            let before = smp.repeat_start - cut_l;
            let inside = cut_r - smp.repeat_start;
            smp.repeat_start -= before;
            smp.repeat_len = smp.repeat_len.saturating_sub(inside).max(4) & !1;
        }
    }

    Some((cut_l, cut_r))
}

fn apply_cosine_fade_out(data: &mut [i8], fade_samples: usize) {
    let len = data.len();
    if len < 4 || fade_samples == 0 {
        return;
    }
    let n = fade_samples.min(len);
    let start = len - n;
    for i in 0..n {
        let t = (i + 1) as f64 / n as f64;
        let gain = 0.5 * (1.0 + (PI * t).cos());
        data[start + i] = ((data[start + i] as f64) * gain).round() as i8;
    }
    // Guarantee exact 0 on last 2 bytes
    data[len - 1] = 0;
    if len >= 2 {
        data[len - 2] = 0;
    }
}
