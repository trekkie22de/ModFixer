# Changelog

All notable changes to **ModFixer** will be documented in this file.

## [0.1.0] - 2026-11-13

### Added
- **Zero-dependency Rust `.MOD` parser & lossless serializer (`src/mod_file.rs`):**
  - Supports 15-sample Ultimate SoundTracker (`ST15`, byte-addressed repeat offsets) and 31-sample ProTracker / NoiseTracker / FastTracker multi-channel modules (`M.K.`, `M!K!`, `FLT4`, `FLT8`, `OCTA`, `OKTA`, `TDZ1`–`TDZ9`, `1CHN`–`32CH`, `.NST`, `.WOW`, `mod.*`).
  - Active Song-Order & Pattern scanner reporting per-sample channel trigger counts (`Ch1..ChN`) and approximate Amiga Paula playback frequency ranges (`Hz`).
- **Forensic defect analyzer (`src/analyzer.rs`):**
  - `EmbeddedHeader`: Detects raw-imported Amiga IFF `8SVX` headers (`FORM..8SVX`, `VHDR`, `NAME`, `ANNO`, `BODY` — including orphaned or nested `BODY` chunks), PC `RIFF WAVE` headers (with unsigned 8-bit PCM detection), and Creative `.VOC` headers.
  - `PostSilenceBurst`: Detects trailing ripper/memory garbage bursts (`±128`) occurring after a one-shot sample has already decayed to silence/DC.
  - `LateTailBurst`: Detects corruption bursts in the decayed final 15% of one-shot samples.
  - `MidSampleBurst`: Detects localized 4-KB floppy/transfer sector corruption islands via opposing slew-rate whiplash statistics while protecting tuned sustain loops (`RepeatStart..RepeatEnd`) and bright sawtooth flybacks.
  - `PseudoOneShotLoop`, `TrailingDeadPadding`, and `AbruptEndCliff` detection.
- **Surgical sample repair engine (`src/fixer.rs`):**
  - Even-word (`2-byte`) aligned tail truncation with raised-cosine fade-outs.
  - Phase- and slope-matched `C¹`-blended splicing across mid-sample corruption bursts.
  - Container header stripping with ProTracker `0x0000` DMA idle word restoration and loop-pointer adjustment.
- **CLI & Inspection Tools (`src/main.rs`, `src/wav_export.rs`):**
  - Single-file and recursive directory `analyze` and `fix` modes (`--only-defects`, `--in-place` with `.bak` backup, `-o <path>`).
  - `--dump-wav <dir>` exporting 16-bit mono RIFF `.wav` before/after comparisons for every modified sample.
- **Cross-Platform Release Build (`build-release.ps1`):**
  - Standalone prebuilt binaries for Windows x64 (`msvc`), Linux x64 (`glibc 2.28`), and Linux ARM64 (`glibc 2.28`).
