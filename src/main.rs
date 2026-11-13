use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mod_fixer::analyzer::{ModuleReport, SampleDefect, analyze_module};
use mod_fixer::fixer::{FixOptions, fix_module};
use mod_fixer::mod_file::ModFile;
use mod_fixer::wav_export::write_sample_wav;

fn print_usage() {
    eprintln!(
        "mod-fixer v{} — Forensic Analyzer & Surgical Sample Repair for .MOD files

USAGE:
  mod-fixer analyze <file.mod | directory> [--only-defects]
  mod-fixer fix <input.mod> [-o <output.mod> | --in-place] [OPTIONS]

OPTIONS:
  -o, --output <path>     Output .MOD path (default: <stem>_fixed.mod)
      --in-place          Overwrite input file (creates <input>.bak backup unless --no-backup)
      --no-backup         Do not create .bak when using --in-place
      --no-strip-headers  Disable stripping embedded IFF 8SVX / RIFF WAVE / VOC headers
      --no-trim-tails     Disable truncating post-silence bursts, late-tail bursts & dead tails
      --no-splice-bursts  Disable splicing mid-sample sector/transfer corruption islands
      --no-fade-cliffs    Disable smoothing abrupt non-zero end cliffs
      --dump-wav <dir>    Export before/after 16-bit WAV files for every repaired sample
      --only-defects      In directory analyze mode, list only modules with detected defects
  -h, --help              Print this help message",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return ExitCode::SUCCESS;
    }

    let (mode, rest) = match args[0].as_str() {
        "analyze" | "scan" => ("analyze", &args[1..]),
        "fix" | "repair" => ("fix", &args[1..]),
        _ => ("analyze", &args[..]),
    };

    let mut target: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut dump_wav: Option<PathBuf> = None;
    let mut in_place = false;
    let mut no_backup = false;
    let mut only_defects = false;
    let mut opts = FixOptions::default();

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i >= rest.len() {
                    eprintln!("Error: missing path after -o/--output");
                    return ExitCode::FAILURE;
                }
                output = Some(PathBuf::from(&rest[i]));
            }
            "--dump-wav" => {
                i += 1;
                if i >= rest.len() {
                    eprintln!("Error: missing directory after --dump-wav");
                    return ExitCode::FAILURE;
                }
                dump_wav = Some(PathBuf::from(&rest[i]));
            }
            "--in-place" => in_place = true,
            "--no-backup" => no_backup = true,
            "--only-defects" => only_defects = true,
            "--no-strip-headers" => opts.strip_headers = false,
            "--no-trim-tails" => opts.trim_tails = false,
            "--no-splice-bursts" => opts.splice_bursts = false,
            "--no-fade-cliffs" => opts.fade_cliffs = false,
            arg if arg.starts_with('-') => {
                eprintln!("Error: unknown flag '{arg}'");
                return ExitCode::FAILURE;
            }
            arg => {
                if target.is_none() {
                    target = Some(PathBuf::from(arg));
                } else {
                    eprintln!("Error: unexpected positional argument '{arg}'");
                    return ExitCode::FAILURE;
                }
            }
        }
        i += 1;
    }

    let Some(target_path) = target else {
        eprintln!("Error: missing target .MOD file or directory");
        print_usage();
        return ExitCode::FAILURE;
    };

    match mode {
        "analyze" => run_analyze(&target_path, only_defects),
        "fix" => run_fix(&target_path, output, in_place, no_backup, opts, dump_wav),
        _ => unreachable!(),
    }
}

fn run_analyze(path: &Path, only_defects: bool) -> ExitCode {
    if path.is_dir() {
        let mut files = Vec::new();
        collect_mod_files(path, &mut files);
        files.sort();

        let mut total_mods = 0usize;
        let mut defective_mods = 0usize;
        let mut total_defects = 0usize;

        for f in &files {
            let Ok(bytes) = fs::read(f) else { continue };
            let Ok(module) = ModFile::parse(&bytes) else {
                continue;
            };
            total_mods += 1;
            let report = analyze_module(&module);
            let d = report.total_defects();
            if d > 0 {
                defective_mods += 1;
                total_defects += d;
                println!(
                    "[DEFECTS: {:2} in {:2} smp] {} ('{}', {})",
                    d,
                    report.affected_samples(),
                    f.display(),
                    report.title,
                    report.format_label
                );
                for sr in &report.sample_reports {
                    for def in &sr.defects {
                        println!(
                            "    Smp {:02} ('{}', {} B): {}",
                            sr.index,
                            sr.name,
                            sr.orig_len,
                            format_defect(def)
                        );
                    }
                }
            } else if !only_defects {
                println!("[CLEAN] {} ('{}')", f.display(), report.title);
            }
        }

        println!(
            "\n=== Scan Summary: {total_mods} valid .MOD files | {defective_mods} with defects ({total_defects} total findings) ==="
        );
        return ExitCode::SUCCESS;
    }

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading '{}': {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    let module = match ModFile::parse(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error parsing '{}': {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    let report = analyze_module(&module);
    print_detailed_report(path, bytes.len(), &report);
    ExitCode::SUCCESS
}

fn run_fix(
    input_path: &Path,
    output_opt: Option<PathBuf>,
    in_place: bool,
    no_backup: bool,
    opts: FixOptions,
    dump_wav: Option<PathBuf>,
) -> ExitCode {
    let bytes = match fs::read(input_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading '{}': {e}", input_path.display());
            return ExitCode::FAILURE;
        }
    };

    let orig_module = match ModFile::parse(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error parsing '{}': {e}", input_path.display());
            return ExitCode::FAILURE;
        }
    };

    let pre_report = analyze_module(&orig_module);
    print_detailed_report(input_path, bytes.len(), &pre_report);

    if pre_report.total_defects() == 0 {
        println!("\nNo sample defects detected — nothing to fix.");
        return ExitCode::SUCCESS;
    }

    let mut fixed_module = orig_module.clone();
    let summary = fix_module(&mut fixed_module, opts);

    if summary.sample_actions.is_empty() {
        println!("\nNo fixes matched active options.");
        return ExitCode::SUCCESS;
    }

    println!("\n=== Applied Surgical Fixes ===");
    for sa in &summary.sample_actions {
        let delta = (sa.new_len as isize) - (sa.orig_len as isize);
        println!(
            "  Smp {:02} ('{}'): {} B -> {} B ({:+} B)",
            sa.sample_index, sa.sample_name, sa.orig_len, sa.new_len, delta
        );
        for act in &sa.actions {
            println!("    + {act}");
        }
    }

    if let Some(wav_dir) = dump_wav {
        if let Err(e) = fs::create_dir_all(&wav_dir) {
            eprintln!("Error creating WAV dump dir '{}': {e}", wav_dir.display());
            return ExitCode::FAILURE;
        }
        for sa in &summary.sample_actions {
            let idx = sa.sample_index - 1;
            let before_wav = wav_dir.join(format!("smp{:02}_before.wav", sa.sample_index));
            let after_wav = wav_dir.join(format!("smp{:02}_after.wav", sa.sample_index));
            let _ = write_sample_wav(&before_wav, &orig_module.samples[idx], 14037);
            let _ = write_sample_wav(&after_wav, &fixed_module.samples[idx], 14037);
        }
        println!("  Exported before/after WAVs (14037 Hz) to '{}'", wav_dir.display());
    }

    let out_bytes = match fixed_module.to_bytes() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error serializing fixed .MOD: {e}");
            return ExitCode::FAILURE;
        }
    };

    let dest_path = if in_place {
        if !no_backup {
            let bak = PathBuf::from(format!("{}.bak", input_path.display()));
            if let Err(e) = fs::copy(input_path, &bak) {
                eprintln!("Error creating backup '{}': {e}", bak.display());
                return ExitCode::FAILURE;
            }
            println!("  Backup written to '{}'", bak.display());
        }
        input_path.to_path_buf()
    } else if let Some(out) = output_opt {
        out
    } else {
        default_fixed_path(input_path)
    };

    if let Err(e) = fs::write(&dest_path, &out_bytes) {
        eprintln!("Error writing '{}': {e}", dest_path.display());
        return ExitCode::FAILURE;
    }

    let post_report = analyze_module(&fixed_module);
    println!(
        "\n=== Result: '{}' written ({} B -> {} B, sample data {:+} B, remaining defects: {}) ===",
        dest_path.display(),
        bytes.len(),
        out_bytes.len(),
        (summary.bytes_after as isize) - (summary.bytes_before as isize),
        post_report.total_defects()
    );

    ExitCode::SUCCESS
}

fn print_detailed_report(path: &Path, file_bytes: usize, report: &ModuleReport) {
    println!("=== Module Analysis: {} ===", path.display());
    println!(
        "  Title: '{}' | Format: {} ({} ch) | Orders: {} | Patterns: {} | Size: {} B",
        report.title,
        report.format_label,
        report.channels,
        report.song_len,
        report.num_patterns,
        file_bytes
    );
    println!("  Samples ({} non-empty):", report.sample_reports.len());

    for sr in &report.sample_reports {
        let loop_info = if sr.is_looped {
            format!("loop {}+{}", sr.repeat_start, sr.repeat_len)
        } else {
            "one-shot".to_string()
        };

        let ch_list: Vec<String> = sr
            .usage
            .channel_triggers
            .iter()
            .enumerate()
            .filter(|&(_, &cnt)| cnt > 0)
            .map(|(ch, &cnt)| format!("Ch{}({cnt}x)", ch + 1))
            .collect();
        let usage_str = if ch_list.is_empty() {
            "unused in active orders".to_string()
        } else {
            let rate_str = match (sr.usage.min_period, sr.usage.max_period) {
                (Some(min_p), Some(max_p)) if min_p > 0 && max_p > 0 => {
                    let min_hz = 3_546_895 / (max_p as u32);
                    let max_hz = 3_546_895 / (min_p as u32);
                    if min_hz == max_hz {
                        format!(" @ ~{min_hz} Hz")
                    } else {
                        format!(" @ ~{min_hz}..{max_hz} Hz")
                    }
                }
                _ => String::new(),
            };
            format!("{}{}", ch_list.join(", "), rate_str)
        };

        let status = if sr.defects.is_empty() { "OK" } else { "DEFECT" };
        println!(
            "  [{status:6}] Smp {:02} '{:<22}' | {:6} B | Vol {:2} | {:<14} | {usage_str}",
            sr.index, sr.name, sr.orig_len, sr.volume, loop_info
        );
        for d in &sr.defects {
            println!("           ! {}", format_defect(d));
        }
    }

    println!(
        "  Summary: {} defect(s) across {} of {} active sample(s).",
        report.total_defects(),
        report.affected_samples(),
        report.sample_reports.len()
    );
}

fn format_defect(d: &SampleDefect) -> String {
    match d {
        SampleDefect::EmbeddedHeader {
            kind,
            strip_bytes,
            summary,
        } => format!(
            "[EmbeddedHeader: {}] strip first {strip_bytes} B ({summary})",
            kind.label()
        ),
        SampleDefect::MidSampleBurst {
            start,
            end,
            max_step,
            ctx_p95_step,
        } => format!(
            "[MidSampleBurst] @{start}..{end} ({} B): maxStep={max_step} vs local p95={ctx_p95_step}",
            end - start + 1
        ),
        SampleDefect::PseudoOneShotLoop {
            repeat_start,
            repeat_len,
        } => format!(
            "[PseudoOneShotLoop] silent/stop loop @{repeat_start}+{repeat_len} on one-shot sample"
        ),
        SampleDefect::PostSilenceBurst {
            truncate_to,
            burst_start,
            burst_end,
            burst_max_step,
        } => format!(
            "[PostSilenceBurst] garbage @{burst_start}..{burst_end} (maxStep={burst_max_step}) after quiet decay -> truncate to {truncate_to} B"
        ),
        SampleDefect::LateTailBurst {
            truncate_to,
            burst_start,
            burst_end,
            burst_max_step,
        } => format!(
            "[LateTailBurst] tail corruption @{burst_start}..{burst_end} (maxStep={burst_max_step}) -> truncate to {truncate_to} B + fade"
        ),
        SampleDefect::TrailingDeadPadding {
            truncate_to,
            dead_bytes,
            dc_value,
        } => format!(
            "[TrailingDeadPadding] {dead_bytes} B constant DC ({dc_value}) at tail -> trim to {truncate_to} B"
        ),
        SampleDefect::AbruptEndCliff {
            tail_peak,
            fade_samples,
        } => format!(
            "[AbruptEndCliff] hard cut-off at amplitude {tail_peak} -> {fade_samples}-sample fade-out"
        ),
    }
}

fn default_fixed_path(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mod");
    parent.join(format!("{stem}_fixed.{ext}"))
}

fn collect_mod_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_mod_files(&p, out);
        } else if is_mod_filename(&p) {
            out.push(p);
        }
    }
}

fn is_mod_filename(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".mod")
        || lower.ends_with(".nst")
        || lower.ends_with(".wow")
        || lower.starts_with("mod.")
}
