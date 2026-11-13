use std::fs;
use std::io;
use std::path::Path;

use crate::mod_file::ModSample;

/// Writes a signed 8-bit `ModSample` as a standard 16-bit mono RIFF `.wav` file
/// (default sample rate 14037 Hz = Amiga A-2, or custom rate) for easy inspection.
pub fn write_sample_wav(path: &Path, smp: &ModSample, sample_rate: u32) -> io::Result<()> {
    let num_samples = smp.data.len();
    let data_bytes = (num_samples * 2) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);

    let mut buf = Vec::with_capacity(44 + num_samples * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM = 1
    buf.extend_from_slice(&1u16.to_le_bytes()); // Mono = 1
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // 16-bit signed PCM
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());

    for &v in &smp.data {
        let s16 = (v as i16) << 8;
        buf.extend_from_slice(&s16.to_le_bytes());
    }

    fs::write(path, buf)
}
