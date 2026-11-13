# ModFixer (`mod-fixer`) — Forensic Analyzer & Surgical Sample Repair for `.MOD` Files

> **Zero-dependency Rust CLI that detects and surgically repairs structural sample corruption in classic 1980s/1990s Amiga & PC Tracker modules (`.MOD`) — artifacts too severe for real-time player de-click filters.**

[English](#why-modfixer) | [Deutsch](#deutsch--kurzbeschreibung) | [Releases](https://github.com/trekkie22de/ModFixer/releases)

---

## Why ModFixer?

Many historical scene, BBS, and disk-swapped `.MOD` files suffer from severe structural sample corruption introduced during late-1980s/early-1990s sampling, ripping, and floppy/modem transfers:

- **Raw-imported container headers (`IFF 8SVX`, `RIFF WAVE`, `.VOC`)** loaded directly as signed 8-bit PCM without stripping the file header — blasting 50–140 bytes of `8SVXVHDR...Audio Master II...BODY` ASCII/binary metadata at full volume on every note attack.
- **Post-silence ripper/memory bursts** where a snare or drum decays cleanly to silence (`0` / `-1`), followed 15 ms later by a full-scale `±128` Chip-RAM garbage spike right at the end of the sample.
- **4-KB sector/transfer corruption islands** (`Δ = 200..255` back-to-back two's-complement polarity wraps) punched right into the middle or decaying tail of smooth synth pads.

Real-time de-click filters in tracker players (such as [MOD4WIN](https://github.com/trekkie22de/Mod4Win) or OpenMPT) are intentionally conservative: they smooth 1–2 ms transitions at note boundaries, loop seams, or the very last few samples, because filtering out 40–130 byte full-scale square bursts in real time would muffle healthy drums, chiptunes, and bright synths.

**ModFixer** solves this offline by analyzing the entire module in context (including pattern channel usage and sustain loop geometry) and performing targeted, bit-clean sample surgery while leaving all pattern data, song orders, and tuned sustain loops (`RepeatStart..RepeatEnd`) completely intact.

### AI-Assisted Development / KI-Transparenzhinweis

- **EN:** In the spirit of full transparency: **ModFixer**—including the forensic `.MOD` sample analysis heuristics, phase-matched zero-crossing splice engine, container header stripper, and test suite—was developed with **Gemini 4 Carbon**.
- **DE:** Transparenzhinweis: **ModFixer** (inklusive der forensischen `.MOD`-Sample-Analyseheuristiken, der phasengleichen Splice-Engine, des Container-Header-Strippers und der Test-Suite) wurde mithilfe von **Gemini 4 Carbon** entwickelt.

---

## Detected & Repaired Defect Classes

| Defect Class | Typical Cause | Surgical Repair Action |
| :--- | :--- | :--- |
| **`EmbeddedHeader`** | Amiga IFF `8SVX` (`FORM..8SVX..BODY`, incl. orphaned or nested `BODY` chunks), PC `RIFF WAVE` (incl. unsigned 8-bit PCM), or Creative `.VOC` loaded as raw PCM | Strips exact header up to PCM payload, converts unsigned WAV PCM if needed, adjusts `RepeatStart`, or writes ProTracker `0x0000` idle word + 6-sample attack ramp on one-shots |
| **`PostSilenceBurst`** | Chip-RAM / SoundTracker ripper garbage (`±128`) sitting *after* `>= 48` samples of clean silence/DC decay ("Snare Syndrome") | Truncates sample on an even 2-byte word boundary inside the quiet floor before the burst + applies a 16-sample raised-cosine fade to `0` |
| **`LateTailBurst`** | Transfer/disk corruption exploding inside the already decayed final 15% of a one-shot pad/synth | Truncates cleanly ahead of the damaged shoulder + applies a smooth 32–128 sample raised-cosine fade-out |
| **`MidSampleBurst`** | Localized 4-KB floppy/modem sector glitches (`Δ <= 255` opposing polarity whiplashes) inside an otherwise smooth waveform | Cuts an even-byte (`2-byte` word-aligned) window between phase- and slope-matched zero-crossings with a 6-sample `C¹` cosine blend. **Never shortens active sustain loops** |
| **`PseudoOneShotLoop`** | Tiny stop-loop or flat silent loop at the end of an effective one-shot sample | Normalizes header to canonical ProTracker one-shot (`RepeatStart = 0`, `RepeatLen = 2`) and trims dead loop tail |
| **`TrailingDeadPadding`** | Excessive non-zero DC tail (`|DC| >= 3`) or `>= 2 KiB` of wasted dead bytes | Trims excess dead tail and fades cleanly to `0` |
| **`AbruptEndCliff`** | Hard mid-wave cutoff (`>= 28` LSB) right at sample end | Applies a gentle 16–48 sample raised-cosine micro-fade to `0` |

### Supported `.MOD` Variants
- **15-Sample Ultimate SoundTracker (`ST15`)** (600-byte header, byte-addressed loop offsets)
- **31-Sample ProTracker / NoiseTracker / FastTracker / TakeTracker** (`M.K.`, `M!K!`, `M&K!`, `N.T.`, `FLT4`, `FLT8`, `OCTA`, `OKTA`, `CD81`, `TDZ1`–`TDZ9`, `1CHN`–`32CH`, `.NST`, `.WOW`, and Amiga `mod.<name>` prefix files)

---

## Real-World Example (`KILLBASS.MOD`)

```text
$ mod-fixer fix KILLBASS.MOD --dump-wav ./wav_compare

=== Module Analysis: KILLBASS.MOD ===
  Title: 'Killbass' | Format: M.K. (4 ch) | Orders: 19 | Patterns: 9 | Size: 55862 B
  Samples (7 non-empty):
  [DEFECT] Smp 01 'st-06:laser3          ' |   1544 B | Vol 64 | one-shot       | Ch3(364x) @ ~27928 Hz
           ! [EmbeddedHeader: Amiga IFF 8SVX] strip first 104 B (IFF 8SVX header + chunks (BODY len=1544), PCM starts at +104)
  [DEFECT] Smp 02 'st-06:mixfx2          ' |  20300 B | Vol 48 | one-shot       | Ch1(36x), Ch2(370x) @ ~5542..31388 Hz
           ! [EmbeddedHeader: Amiga IFF 8SVX] strip first 104 B (IFF 8SVX header + chunks (BODY len=20390), PCM starts at +104)
           ! [LateTailBurst] tail corruption @18806..18876 (maxStep=220) -> truncate to 18806 B + fade
           ! [MidSampleBurst] @6546..6594 (49 B): maxStep=195 vs local p95=80
           ! [MidSampleBurst] @10645..10687 (43 B): maxStep=231 vs local p95=80
           ! [MidSampleBurst] @14761..14779 (19 B): maxStep=162 vs local p95=69
  [DEFECT] Smp 03 'st-06:snare           ' |   2752 B | Vol 64 | one-shot       | Ch3(177x) @ ~11084 Hz
           ! [PostSilenceBurst] garbage @2622..2671 (maxStep=246) after quiet decay -> truncate to 2472 B
  [OK    ] Smp 04 '                      ' |   1684 B | Vol 64 | one-shot       | Ch3(140x) @ ~16574 Hz
  [DEFECT] Smp 05 'st-06:toma            ' |   1226 B | Vol 64 | one-shot       | Ch1(12x), Ch2(7x), Ch3(39x), Ch4(10x) @ ~4143..15694 Hz
           ! [EmbeddedHeader: Amiga IFF 8SVX] strip first 14 B (Orphaned IFF BODY chunk header (len=1210), PCM starts at +14)
  [DEFECT] Smp 06 'st-05:bass7           ' |   2056 B | Vol 53 | one-shot       | Ch3(38x), Ch4(832x) @ ~6592..8287 Hz
           ! [EmbeddedHeader: Amiga IFF 8SVX] strip first 132 B (IFF 8SVX header + chunks (BODY len=2032), PCM starts at +132)
  [DEFECT] Smp 08 'st-06:shagonyfx       ' |  16000 B | Vol 38 | one-shot       | Ch1(181x), Ch2(1x), Ch4(56x) @ ~6982..13964 Hz
           ! [EmbeddedHeader: Amiga IFF 8SVX] strip first 104 B (IFF 8SVX header + chunks (BODY len=16274), PCM starts at +104)
  Summary: 10 defect(s) across 6 of 7 active sample(s).

=== Result: 'KILLBASS_fixed.MOD' written (55862 B -> 53428 B, sample data -2434 B, remaining defects: 0) ===
```

---

## Usage

Download a prebuilt binary for **Windows x64**, **Linux x64**, or **Linux ARM64** from [Releases](https://github.com/trekkie22de/ModFixer/releases), or build from source (`cargo build --release`).

```bash
# 1. Inspect a single .MOD file (read-only forensic report + channel usage)
mod-fixer analyze track.mod

# 2. Scan an entire archive directory recursively and list only defective modules
mod-fixer analyze /path/to/mods --only-defects

# 3. Repair a single file (writes track_fixed.mod by default, leaving original untouched)
mod-fixer fix track.mod

# 4. Repair with custom output path + export before/after 16-bit WAVs of modified samples
mod-fixer fix track.mod -o track_clean.mod --dump-wav ./wav_compare

# 5. Batch-repair all defective .MODs in a folder tree into a separate output directory
mod-fixer fix /path/to/mods -o /path/to/repaired_mods

# 6. Repair in-place (automatically writes track.mod.bak unless --no-backup is set)
mod-fixer fix track.mod --in-place
```

Individual repair stages can be selectively disabled via `--no-strip-headers`, `--no-trim-tails`, `--no-splice-bursts`, and `--no-fade-cliffs`.

---

## Deutsch – Kurzbeschreibung

**ModFixer (`mod-fixer`)** ist ein schlankes, abhängigkeitsfreies Rust-Kommandozeilenwerkzeug zur forensischen Analyse und chirurgischen Reparatur kaputter Samples in klassischen Amiga- und PC-`.MOD`-Dateien (15-Sample SoundTracker sowie 31-Sample ProTracker / Multi-Channel).

Es behebt genau jene massiven Sample-Defekte alter Szene- und Disketten-Module, gegen die ein Echtzeit-De-Click-Filter im Player machtlos ist:
1. **Unentfernte Container-Header am Sample-Anfang** (`Amiga IFF 8SVX`, `RIFF WAVE` inkl. Unsigned-Konvertierung, `Creative VOC`), die sonst bei jedem Tonanschlag als lautes digitales Kratzen mitgespielt werden.
2. **Ripper-/Chip-RAM-Müllsalven am Sample-Ende** (z. B. Snares oder Drums, die längst auf `0` abgeklungen sind und kurz vor Schluss noch einen `±128`-Knaller abfeuern) durch wortgenaues Einkürzen + sanften Kosinus-Fade.
3. **4-KB-Sektor-/Übertragungsschäden mitten in Samples** durch phasen- und steigungsgleiches Herausschneiden im 2-Byte-Wortraster (`C¹`-Splice), wobei gestimmte Sustain-Loops (`RepeatStart..RepeatEnd`) strikt geschützt bleiben, damit sich kein Instrument verstimmt.

---

## Building from Source

Requires Rust 1.85+ (`edition = "2024"`). Zero external crate dependencies.

```bash
cargo build --release
cargo test
```

## License

Released under the [MIT License](LICENSE).
