# ModFixer (`mod-fixer`)

Forensisches Analyse- und Sample-Reparatur-Werkzeug in reinem Rust (ohne externe Laufzeit-Abhängigkeiten) für klassische Amiga-/PC-Tracker-Module (`.MOD`, 15-Sample Ultimate SoundTracker sowie 31-Sample ProTracker / NoiseTracker / FastTracker `M.K.`, `FLT4/8`, `4CHN`–`32CH`).

Entstanden aus der Analyse historisch beschädigter Szene-Module (wie `KILLBASS.MOD`), bei denen rohe Container-Header, 4-KB-Sektor-Übertragungsfehler oder Müll-Bursts am Sample-Ende mit klassischen Echtzeit-De-Click-Filtern im Player prinzipbedingt nicht sauber beherrschbar sind.

---

## Erkannte & reparierte Defektklassen

1. **Eingebackene Fremd-Header am Sample-Anfang (`EmbeddedHeader`)**
   - Erkennt als Raw-PCM eingeladene **Amiga IFF `8SVX`**-Header (`FORM..8SVX`, `VHDR`, `NAME`, `ANNO`, `BODY` – inkl. verwaister oder doppelt verschachtelter `BODY`-Chunks), **RIFF `WAVE`** (inkl. automatischer Konvertierung von unsigned 8-Bit PCM auf signed 8-Bit) sowie **Creative `.VOC`**.
   - Schneidet den Header exakt bis zum PCM-Start ab, korrigiert `RepeatStart` bei geloopten Samples bzw. setzt bei One-Shot-Samples das ProTracker-konforme Idle-Wort (`0x0000`) samt kurzem Attack-Fade-In.

2. **End-Müll nach Stille (`PostSilenceBurst`, „Snare-Syndrom“)**
   - Fällt ein One-Shot-Sample nach dem Hauptsignal über `>= 48` Samples auf den Ruhepegel (`0` / konstantes Klein-DC) und folgt *danach* noch ein isolierter Müll-/Ripper-Burst (`±128`), wird das Sample wortgenau (`2 Bytes`) in der Stille vor dem Ausbruch eingekürzt und sanft auf `0` ausgeblendet.

3. **Korruptions-Explosionen im späten Ausklang (`LateTailBurst`, „Pad-Tail-Syndrom“)**
   - Tritt in den letzten 15 % eines bereits deutlich abgeklungenen One-Shot-Samples ein plötzlicher Sektor-/Clipping-Ausbruch auf, wird das Sample unmittelbar vor der Schadstelle eingekürzt und mit einer Kosinus-Rampe weich ausgeblendet.

4. **Lokale Sektor-/Transfer-Bursts mitten im Sample (`MidSampleBurst`)**
   - Erkennt eng begrenzte Korruptionsinseln (z. B. 4-KB-Disketten-/Transfer-Schäden mit gegenläufigen Full-Scale-Wraps) inmitten glatter Wellenformen und schneidet das defekte Fenster an **phasen- und steigungsgleichen Stützstellen** im 2-Byte-Wortraster (`C¹`-geglätteter Splice) heraus.
   - **Loop-Schutz:** Gestimmt geloopte Sustain-Bereiche (`RepeatStart .. RepeatStart + RepeatLen`) werden niemals verkürzt, damit keine Tonhöhenverschiebung entsteht.

5. **Pseudo-Loops, totes DC-Padding & harte Abbruchkanten (`PseudoOneShotLoop`, `TrailingDeadPadding`, `AbruptEndCliff`)**
   - Bereinigt stille Pseudo-Endloops auf kanonische One-Shots (`0 + 2`), kürzt langes DC-behaftetes End-Padding ein und glättet harte Mid-Wave-Abbrüche (`>= 28` LSB) am Sample-Ende.

---

## Build & Verwendung

```bash
cargo build --release
```

### 1. Einzeldatei oder komplettes Archiv analysieren (Read-Only)

```bash
# Detaillierter Forensik-Bericht inkl. Kanal-Nutzung & Frequenzbereich pro Sample
mod-fixer analyze KILLBASS.MOD

# Rekursiver Scan über ein Verzeichnis (nur betroffene Module listen)
mod-fixer analyze /mnt/mods/MOD --only-defects
```

### 2. Modul reparieren

```bash
# Schreibt standardmäßig KILLBASS_fixed.MOD (Original bleibt unangetastet)
mod-fixer fix KILLBASS.MOD

# Expliziter Ausgabepfad + Vorher/Nachher-WAV-Export (16-Bit PCM) aller geänderten Samples
mod-fixer fix KILLBASS.MOD -o KILLBASS_fixed.MOD --dump-wav ./wav_compare

# In-Place-Reparatur mit automatischem .bak-Backup
mod-fixer fix KILLBASS.MOD --in-place
```

Einzelne Reparaturstufen lassen sich gezielt abschalten (`--no-strip-headers`, `--no-trim-tails`, `--no-splice-bursts`, `--no-fade-cliffs`).
